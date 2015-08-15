#![feature(plugin)]
#![plugin(regex_macros)]

extern crate argparse;
extern crate html5ever;
extern crate html5ever_dom_sink;
extern crate hyper;
extern crate iron;
#[macro_use]
extern crate log;
extern crate log4rs;
extern crate redis;
extern crate regex;
extern crate rustc_serialize;
extern crate tempfile;
extern crate time;
extern crate url;

// To mark areas of the merged text that were merged in from vandalized edits, the code uses
// placeholder characters at the start and end of each merged region.
//
// These two characters are taken from a Unicode Private Use Area, so they should never appear in
// actual Wikipedia text.
const START_MARKER: &'static str = "\u{E000}";
const END_MARKER: &'static str = "\u{E001}";

/// Helper macro for unwrapping Result values whose E types implement std::fmt::Display. For Ok(),
/// evaluates to the contained value. For Err(), returns early with an Err containing the formatted
/// error.
macro_rules! try_display {
    ($expr:expr, $($format_arg:expr),* ) => (match $expr {
        Ok(val) => val,
        Err(err) => return Err(format!("{}: {}", format!($($format_arg),*), err)),
    })
}

/// Helper macro for unwrapping Result values whose E types implement std::fmt::Display, and that
/// are being unwrapped in functions that don't return Result. For Ok(), evaluates to the contained
/// value. For Err(), logs the formatted error at with error!(), and returns the value in the second
/// argument.
macro_rules! try_return {
    ($expr:expr, $retval:expr, $($format_arg:expr),* ) => (match $expr {
        Ok(val) => val,
        Err(err) => {
            error!("{}: {}", format!($($format_arg),*), err);
            return $retval;
        },
    })
}

mod json;
mod longest_common_subsequence;
mod merge;
mod page;
mod wiki;

use argparse::ArgumentParser;
use argparse::Store;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::iter::FromIterator;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;

use hyper::Client;
use hyper::header::Connection;
use iron::Iron;
use iron::IronResult;
use iron::Request;
use iron::Response;
use iron::headers::ContentType;
use iron::middleware::Handler;
use iron::mime::Mime;
use iron::mime::SubLevel;
use iron::mime::TopLevel;
use tempfile::NamedTempFile;

use page::Page;
use wiki::Revision;
use wiki::Wiki;

// TODO: consider doing s/en.wikipedia.org/this app's url/ on the HTML before serving it. This
// currently works fine, but might not over HTTPS.

/// A struct that uses RAII to log durations: when dropped, it logs the number of milliseconds it
/// existed, prefixed by `name`.
struct Timer {
    name: String,
    start_time_ns: u64
}

impl Timer {
    fn new(name: String) -> Timer {
        Timer {
            name: name,
            start_time_ns: time::precise_time_ns(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        info!("{}: {} ms", self.name, (time::precise_time_ns() - self.start_time_ns) / 1_000_000);
    }
}

fn spawn_threads<I>(sections: I) ->
    (HashMap<String, Sender<Option<(String, String, u64)>>>, HashMap<String, Receiver<String>>)
    where I: IntoIterator<Item=(String, String)> {
    let mut senders_map = HashMap::new();
    let mut receivers_map = HashMap::new();
    for (section_title, section_content) in sections.into_iter() {
        let (in_sender, in_receiver) = channel::<Option<(String, String, u64)>>();
        let (out_sender, out_receiver) = channel::<String>();
        // TODO: delete
        let section_t = section_title.clone();
        thread::spawn(move|| {
            let mut merged_content = section_content;
            let _timer = Timer::new(format!("Merged all revisions of \"{}\"", section_t));
            loop {
                match in_receiver.recv() {
                    Ok(Some((clean_content, vandalized_content, revision_id))) => {
                        merged_content = merge::try_merge(
                            &clean_content, &merged_content, &vandalized_content,
                            &revision_id.to_string());
                    },
                    Ok(None) => {
                        out_sender.send(merged_content);
                        drop(_timer);
                        break;
                    },
                    Err(..) => panic!("Failed to receive from in_receiver"),
                }
            }
        });
        senders_map.insert(section_title.clone(), in_sender);
        receivers_map.insert(section_title, out_receiver);
    }
    (senders_map, receivers_map)
}

struct WikipediaMinusWikipediansHandler {
    wiki: Wiki,
    client: Client
}

impl WikipediaMinusWikipediansHandler {
    fn new(wiki: Wiki, client: Client) -> WikipediaMinusWikipediansHandler {
        WikipediaMinusWikipediansHandler { wiki: wiki, client: client }
    }

    /// Returns a vector of Revisions representing all reversions of vandalism for the page `title`.
    fn get_antivandalism_revisions(&self, title: &str) -> Result<Vec<Revision>, String> {
        let revisions = try!(self.wiki.get_revisions(title, 500));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    /// Fetches each specified revision of the page `title`, parses it into sections, and sends each
    /// section's content to the Sender associated with the section's title in `senders`.
    fn fetch_revisions_content(
        &self, title: String, revisions: Vec<Revision>,
        senders: HashMap<String, Sender<Option<(String, String, u64)>>>) -> Result<(), String> {
        let _timer =
            Timer::new(format!("Got content of {} revisions of \"{}\"", revisions.len(), title));
        let mut receivers: Vec<(u64, Receiver<Result<Vec<(String, String)>, String>>,
                                Receiver<Result<Vec<(String, String)>, String>>)> =
            Vec::with_capacity(revisions.len());
        for revision in &revisions {
            let mut inner_receivers = Vec::new();
            for revision_id in vec![revision.revid, revision.parentid] {
                let (sender, receiver) = channel();
                let wiki = self.wiki.clone();
                let title = title.to_string().clone();
                let revision = revision.clone();
                thread::spawn(move|| {
                    sender.send(
                        match wiki.get_revision_content(&title, revision_id) {
                            Ok(content) => Ok(wiki::parse_sections(&content)),
                            _ => Err(format!(
                                "Failed to get content of revision {} of \"{}\"", revision_id,
                                title)),
                        }).unwrap();
                });
                inner_receivers.push(receiver);
            }
            receivers.push(
                (revision.parentid, inner_receivers.remove(0), inner_receivers.remove(0)));
        }

        for (revision_id, clean_receiver, vandalized_receiver) in receivers {
            let mut clean_sections: HashMap<String, String> =
                HashMap::from_iter(
                    try!(try_display!(clean_receiver.recv(), "Failed to get data from thread")));
            let mut vandalized_sections: HashMap<String, String> =
                HashMap::from_iter(try!(
                    try_display!(vandalized_receiver.recv(), "Failed to get data from thread")));

            for (title, sender) in senders.iter() {
                match (clean_sections.remove(title), vandalized_sections.remove(title)) {
                    (Some(clean_content), Some(vandalized_content)) => {
                        sender.send(Some((clean_content, vandalized_content, revision_id)));
                    },
                    _ => (),
                }
            }
        }
        for sender in senders.values() {
            sender.send(None);
        }

        Ok(())
    }

    fn get_page_with_vandalism_restored(&self, title: &str) -> Result<String, String> {
        let page = Page::new(title, self.wiki.clone());

        // TODO: This almost surely doesn't need to be an Arc.
        let canonical_title = Arc::new(try!(self.wiki.get_canonical_title(title)));
        info!("Canonical page title for \"{}\" is \"{}\"", title, canonical_title);

        let latest_revision = try!(self.wiki.get_latest_revision(&canonical_title));
        let latest_revision_content =
                try!(self.wiki.get_revision_content(&canonical_title, latest_revision.revid));
        let latest_revision_sections = wiki::parse_sections(&latest_revision_content);

        let (senders, receivers) = spawn_threads(latest_revision_sections.clone());
        let antivandalism_revisions = try!(self.get_antivandalism_revisions(&canonical_title));

        let _timer = Timer::new(format!("Fetched and merged {} revisions of \"{}\"",
                                        (&antivandalism_revisions).len(), title));
        try!(self.fetch_revisions_content(
            (*canonical_title).clone(), antivandalism_revisions, senders));
        // TODO: get this working, instead of the for loop below
        //let merged_content =
        //    latest_revision_sections.into_iter().map(
        //        |section_title, _| channels.get(&section_title).unwrap().1.recv().unwrap())
        //    .join("");
        let mut merged_content = String::new();
        for (section_title, _) in latest_revision_sections {
            let received_value = receivers.get(&section_title).unwrap().recv().unwrap();
            merged_content.push_str(&received_value);
        }
        drop(_timer);

        let html_body = try!(self.wiki.parse_wikitext(&canonical_title, &merged_content));

        let _marker_timer = Timer::new("Mangled HTML".to_string());
        page.replace_body_and_remove_merge_markers(html_body)
    }
}

impl Handler for WikipediaMinusWikipediansHandler {
    fn handle(&self, request: &mut Request) -> IronResult<Response> {
        if request.url.path.len() == 2 && request.url.path[0] == "wiki" {
            let _timer = Timer::new(format!("Served request for /wiki/{}", request.url.path[1]));
            let mut response =
                match self.get_page_with_vandalism_restored(&request.url.path[1]) {
                    Ok(page_contents) => Response::with((iron::status::Ok, page_contents)),
                    // TODO: create an Error type to pass around, so this can distinguish different
                    // types of error (if that would be helpful).
                    // TODO: create a better error page
                    Err(msg) => {
                        warn!("Failed to get page with vandalism restored: {}", msg);
                        Response::with(
                            (iron::status::InternalServerError, "<html><body>ERROR</body></html>"))
                    },
                };
            response.headers.set(ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
            Ok(response)
        } else {
            // TODO: should I use an HTTP redirect here instead? Would that work? Would it be desirable?
            // TODO: Maybe should be moved to wiki module.
            let mut url = request.url.clone();
            url.scheme = "https".to_string();
            url.host = url::Host::Domain(self.wiki.hostname.clone());
            url.port = self.wiki.port;
            let url = url.into_generic_url().serialize();
            match self.client.get(&url)
                .header(Connection::close()).send() {
                    Ok(mut wikipedia_response) => {
                        let mut wikipedia_body: Vec<u8> = Vec::new();
                        match wikipedia_response.read_to_end(&mut wikipedia_body) {
                            Ok(..) => {
                                info!("Received {} response from {}", wikipedia_response.status,
                                      url);
                                let mut response = Response::with(wikipedia_body);
                                response.status = Some(wikipedia_response.status);
                                response.headers = wikipedia_response.headers.clone();
                                Ok(response)
                            },
                            Err(error) => {
                                warn!("Error reading Wikipedia response: {}", error);
                                let mut response = Response::with(
                                    (iron::status::InternalServerError,
                                     "<html><body>ERROR</body></html>"));
                                response.headers.set(
                                    ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
                                Ok(response)
                            }
                        }
                    },
                    Err(error) => {
                        warn!("Error reading URL {}: {}", url, error);
                        let mut response = Response::with(
                            (iron::status::InternalServerError,
                             "<html><body>ERROR: {}</body></html>"));
                        response.headers.set(
                            ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
                        Ok(response)
                    }
                }
        }
    }
}

fn main() {
    log4rs::init_file("log.toml", Default::default()).unwrap();

    let mut port = 3000;
    let mut wiki = "en.wikipedia.org".to_string();
    let mut redis_hostname = "".to_string();
    let mut redis_port = 6379;
    {
        let mut parser = ArgumentParser::new();
        parser.set_description("TODO: Usage description");
        parser.refer(&mut port).add_option(&["-p", "--port"], Store, "The port to serve HTTP on.");
        parser.refer(&mut wiki).add_option(
            &["--wiki"], Store, "The hostname or hostname:port of the wiki to mirror.");
        parser.refer(&mut redis_hostname).add_option(
            &["--redis_hostname"], Store,
            "The hostname of the Redis server to use. Leave blank to disable Redis.");
        parser.refer(&mut redis_port).add_option(
            &["--redis_port"], Store,
            "The port of the Redis server to use. Ignored if --redis_hostname is blank.");
        parser.parse_args_or_exit();
    }
    let mut wiki_components = wiki.split(":");
    let wiki_hostname = wiki_components.next().unwrap();
    let wiki_port = match wiki_components.next() {
        Some(port) => port.parse::<u16>().unwrap(),
        None => 443,
    };

    let redis_connection_info = if redis_hostname == "" {
        None
    } else {
        Some(redis::ConnectionInfo {
            addr: Box::new(redis::ConnectionAddr::Tcp(redis_hostname, redis_port)),
            db: 0,
            passwd: None,
        })
    };

    let handler =
        WikipediaMinusWikipediansHandler::new(
            Wiki::new(wiki_hostname.to_string(), wiki_port, Client::new(), redis_connection_info),
            Client::new());
    Iron::new(handler).http(("localhost", port)).unwrap();
}
