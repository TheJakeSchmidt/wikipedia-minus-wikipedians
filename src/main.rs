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
extern crate rand;
extern crate redis;
extern crate regex;
extern crate rustc_serialize;
extern crate tempfile;
extern crate tendril;
extern crate time;
extern crate url;

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
mod wiki;

use argparse::ArgumentParser;
use argparse::Store;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;

use html5ever::Attribute;
use html5ever::tree_builder::interface::TreeSink;
use html5ever_dom_sink::common::NodeEnum;
use html5ever_dom_sink::rcdom::Handle;
use html5ever_dom_sink::rcdom::RcDom;
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

// TODO: document. And, ideally, make this function less dumb.
// TODO: return (&str, &str)
fn create_sections_map<I>(arg: I) -> HashMap<String, String>
    where I: IntoIterator<Item=(String, String)> {
    let mut map = HashMap::new();
    for (section_title, section_contents) in arg.into_iter() {
        map.insert(section_title, section_contents);
    }
    map
}

fn spawn_threads<I>(sections: I) ->
    HashMap<String, (Sender<Option<(String, String)>>, Receiver<String>)>
    where I: IntoIterator<Item=(String, String)> {
    let mut channels_map = HashMap::new();
    for (section_title, section_content) in sections.into_iter() {
        let (in_sender, in_receiver) = channel::<Option<(String, String)>>(); 
        let (out_sender, out_receiver) = channel::<String>();
        // TODO: delete
        let section_t = section_title.clone();
        thread::spawn(move|| {
            let mut merged_content = section_content;
            let _timer = Timer::new(format!("Merged all revisions of \"{}\"", section_t));
            loop {
                match in_receiver.recv() {
                    Ok(Some((clean_content, vandalized_content))) => {
                        merged_content = merge::try_merge(
                            &clean_content, &merged_content, &vandalized_content);
                    },
                    Ok(None) => {
                        out_sender.send(merged_content);
                        break;
                    },
                    Err(..) => panic!("Failed to receive from in_receiver"),
                }
            }
        });
        channels_map.insert(section_title, (in_sender, out_receiver));
    }
    channels_map
}

fn replace_node_with_placeholder(original_html: &str, div_id: &str, placeholder: &str)
    -> Result<String, String> {
    let html = tendril::StrTendril::from_str(original_html).unwrap();
    let mut dom: RcDom = html5ever::parse(html5ever::one_input(html), Default::default());

    let handle = try!(find_node_by_id(&dom.get_document(), div_id));
    let child_handles =
        (&handle.borrow().children).into_iter().map(|child| child.clone()).collect::<Vec<_>>();
    for child_handle in child_handles {
        dom.remove_from_parent(child_handle);
    }
    dom.append(handle,
               html5ever::tree_builder::interface::NodeOrText::AppendText(
                   tendril::StrTendril::from_str(placeholder).unwrap()));
    let mut serialized: Vec<u8> = vec![];
    try_display!(
        html5ever::serialize::serialize(&mut serialized, &dom.document, Default::default()),
        "Failed to serialize modified HTML");
    Ok(try_display!(String::from_utf8(serialized),
                    "Error converting serialized HTML to UTF-8 string"))
}

fn find_node_by_id(handle: &Handle, id: &str) -> Result<Handle, String> {
    fn has_matching_id(attributes: &Vec<Attribute>, id: &str) -> bool {
        return attributes.into_iter().any(
            |attribute| attribute.name.local.as_slice() == "id" &&
                format!("{}", attribute.value) == id);
    }

    let node = handle.borrow();
    match node.node {
        NodeEnum::Element(_, ref attributes) if has_matching_id(attributes, id) => Ok(handle.clone()),
        _ => (&node.children).into_iter()
            .map(|child| find_node_by_id(child, id))
            .filter(|result| result.is_ok())
            .map(|result| result.unwrap())
            .next().ok_or(format!("No node with ID {} found", id)),
    }
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
        let revisions = try!(self.wiki.get_revisions(title, 100));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    /// Returns a vector of tuples containing the contents of each revision and its parent revision.
    fn get_revisions_content(&self, title: String, revisions: Vec<Revision>)
                             -> Result<Vec<(Vec<(String, String)>, Vec<(String, String)>)>, String> {
        let _timer =
            Timer::new(format!("Got content of {} revisions of \"{}\"", revisions.len(), title));
        let mut receivers: Vec<Receiver<(Result<String, String>, Result<String, String>)>> =
            Vec::with_capacity(revisions.len());
        for revision in &revisions {
            let (sender, receiver) = channel();
            let wiki = self.wiki.clone();
            let title = title.to_string().clone();
            let revision = revision.clone();
            thread::spawn(move|| {
                // TODO: check result?
                sender.send(
                    (wiki.get_revision_content(&title, revision.revid),
                     wiki.get_revision_content(&title, revision.parentid)));
            });
            receivers.push(receiver);
        }

        // Elements: (revision content, parent revision content)
        let mut revisions_content: Vec<(Vec<(String, String)>, Vec<(String, String)>)> =
            Vec::with_capacity(revisions.len());
        for receiver in receivers {
            let (clean_result, vandalized_result) =
                try_display!(receiver.recv(), "Failed to get data from thread");
            // TODO: I don't know why &(try!(clean_result)) doesn't work in the parse_sections()
            // call. Investigate.
            let clean_result = try!(clean_result);
            let vandalized_result = try!(vandalized_result);
            revisions_content.push((wiki::parse_sections(&clean_result),
                                    wiki::parse_sections(&vandalized_result)));
        }

        Ok(revisions_content)
    }

    fn get_page_with_vandalism_restored(&self, title: &str) -> Result<String, String> {
        let canonical_title = Arc::new(try!(self.wiki.get_canonical_title(title)));
        info!("Canonical page title for \"{}\" is \"{}\"", title, canonical_title);

        let antivandalism_revisions = try!(self.get_antivandalism_revisions(&canonical_title));
        let antivandalism_revisions_sections =
            try!(self.get_revisions_content((*canonical_title).clone(), antivandalism_revisions));

        let latest_revision = try!(self.wiki.get_latest_revision(&canonical_title));
        let latest_revision_content =
                try!(self.wiki.get_revision_content(&canonical_title, latest_revision.revid));
        let latest_revision_sections = wiki::parse_sections(&latest_revision_content);

        let _merge_timer = Timer::new(format!("Merged {} revisions of \"{}\"",
                                              (&antivandalism_revisions_sections).len(), title));

        let channels: HashMap<String, (Sender<_>, Receiver<_>)> =
            spawn_threads(latest_revision_sections.clone());
        for (clean_sections, vandalized_sections) in antivandalism_revisions_sections {
            let mut clean_sections_map = create_sections_map(clean_sections);
            let mut vandalized_sections_map = create_sections_map(vandalized_sections);
            for section in &latest_revision_sections {
                let section_title = &section.0;
                match (clean_sections_map.remove(section_title),
                       vandalized_sections_map.remove(section_title)) {
                    (Some(clean_section_content), Some(vandalized_section_content)) => {
                        channels.get(section_title).unwrap().0.send(
                            Some((clean_section_content, vandalized_section_content)));
                    }
                    _ => (),
                }
            }
        }

        // TODO: can't get "for (sender, _) in channels.values()" to work. Fix it.
        for val in channels.values() {
            val.0.send(None);
        }

        // TODO: get this working, instead of the for loop below
        //let merged_content =
        //    latest_revision_sections.into_iter().map(
        //        |section_title, _| channels.get(&section_title).unwrap().1.recv().unwrap())
        //    .join("");
        let mut merged_content = String::new();
        for (section_title, _) in latest_revision_sections {
            let received_value = channels.get(&section_title).unwrap().1.recv().unwrap();
            merged_content.push_str(&received_value);
        }

        drop(_merge_timer);

        let html_body = try!(self.wiki.parse_wikitext(&canonical_title, &merged_content));
        // Note: "title" rather than "canonical_title", so that redirects look right.
        let current_page_contents = try!(self.wiki.get_current_page_content(&title));
        let placeholder = format!("WMW_PLACEHOLDER_TEXT_{}", rand::random::<u64>());
        let page_contents_with_placeholder =
            try!(replace_node_with_placeholder(
                &current_page_contents, "mw-content-text", &placeholder));
        Ok(page_contents_with_placeholder.replace(&placeholder, &html_body))
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

#[cfg(test)]
mod tests {
    use super::{replace_node_with_placeholder};

    #[test]
    fn test_replace_html_content() {
        let original_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\"><p>original text</p></div><div>Other text</div></div></div></body></html>";
        let expected_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\">replaced text</div><div>Other text</div></div></div></body></html>";
        let processed_html = replace_node_with_placeholder(original_html, "mw-content-text", "replaced text").unwrap();
        assert_eq!(expected_html, processed_html);
    }
}
