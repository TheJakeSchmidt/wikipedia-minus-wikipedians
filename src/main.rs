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
extern crate url;

use argparse::ArgumentParser;
use argparse::Store;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::iter::FromIterator;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, Sender};
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

use merge::Merger;
use page::Page;
use timer::Timer;
use wiki::Revision;
use wiki::Wiki;

/// To mark areas of the merged text that were merged in from vandalized edits, the code uses
/// placeholder characters at the start and end of each merged region.
///
/// These two characters are taken from a Unicode Private Use Area, so they should never appear in
/// actual Wikipedia text.
const START_MARKER: &'static str = "\u{E000}";
const END_MARKER: &'static str = "\u{E001}";

/// See the documentation for `deduplicate_section_titles` for a description of how this constant is
/// used.
const TITLE_COUNT_SEPARATOR: &'static str = "\u{E002}";

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
mod timer;
mod wiki;

// TODO: consider doing s/en.wikipedia.org/this app's url/ on the HTML before serving it. This
// currently works fine, but might not over HTTPS.

struct WikipediaMinusWikipediansHandler {
    wiki: Wiki,
    client: Client,
    merger: Merger,
    max_consecutive_diff_timeouts: u64,
}

impl WikipediaMinusWikipediansHandler {
    fn new(wiki: Wiki, client: Client, merger: Merger, max_consecutive_diff_timeouts: u64) ->
        WikipediaMinusWikipediansHandler {
        WikipediaMinusWikipediansHandler {
            wiki: wiki,
            client: client,
            merger: merger,
            max_consecutive_diff_timeouts: max_consecutive_diff_timeouts,
        }
    }

    /// Returns a vector of Revisions representing all reversions of vandalism for the page `title`.
    fn get_antivandalism_revisions(&self, title: &str) -> Result<Vec<Revision>, String> {
        let revisions = try!(self.wiki.get_revisions(title, 500));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    /// Fetches each specified revision of the page `title`, parses it into sections, and sends each
    /// section's content to the Sender associated with the section's title in
    /// `revision_content_senders`.
    fn fetch_revisions_content(
        &self, title: String, revisions: Vec<Revision>,
        revision_content_senders: HashMap<String, Sender<Option<(String, String, u64)>>>)
        -> Result<(), String> {
        let _timer =
            Timer::new(format!("Got content of {} revisions of \"{}\"", revisions.len(), title));
        // Elements are (clean revision ID, receiver for clean revision content, receiver for
        // vandalized revision content).
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
                thread::Builder::new().name(format!("fetch-content-{}-{}", title, revision_id))
                    .spawn(move|| {
                        sender.send(
                            match wiki.get_revision_content(&title, revision_id) {
                                Ok(content) =>
                                    Ok(deduplicate_section_titles(wiki::parse_sections(&content))),
                                _ => Err(format!(
                                    "Failed to get content of revision {} of \"{}\"", revision_id,
                                    title)),
                            }).unwrap();
                    });
                inner_receivers.push(receiver);
            }
            receivers.push(
                (revision.revid, inner_receivers.remove(0), inner_receivers.remove(0)));
        }

        for (revision_id, clean_receiver, vandalized_receiver) in receivers {
            let mut clean_sections: HashMap<String, String> =
                HashMap::from_iter(
                    try!(try_display!(clean_receiver.recv(), "Failed to get data from thread")));
            let mut vandalized_sections: HashMap<String, String> =
                HashMap::from_iter(try!(
                    try_display!(vandalized_receiver.recv(), "Failed to get data from thread")));

            for (title, revision_content_sender) in revision_content_senders.iter() {
                match (clean_sections.remove(title), vandalized_sections.remove(title)) {
                    (Some(clean_content), Some(vandalized_content)) => {
                        revision_content_sender.send(
                            Some((clean_content, vandalized_content, revision_id)));
                    },
                    _ => (),
                }
            }
        }
        for revision_content_sender in revision_content_senders.values() {
            revision_content_sender.send(None);
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
        let latest_revision_sections =
            deduplicate_section_titles(wiki::parse_sections(&latest_revision_content));

        let (revision_content_senders, merged_content_receivers) =
            self.spawn_merge_threads(title, latest_revision_sections.clone());
        let antivandalism_revisions = try!(self.get_antivandalism_revisions(&canonical_title));

        let _timer = Timer::new(format!("Fetched and merged {} revisions of \"{}\"",
                                        (&antivandalism_revisions).len(), title));
        try!(self.fetch_revisions_content(
            (*canonical_title).clone(), antivandalism_revisions, revision_content_senders));
        // TODO: get this working, instead of the for loop below
        //let merged_article =
        //    latest_revision_sections.into_iter().map(
        //        |section_title, _|
        //        merged_content_receivers.get(&section_title).unwrap().1.recv().unwrap())
        //    .join("");
        let mut merged_article = String::new();
        for (section_title, _) in latest_revision_sections {
            let merged_section =
                merged_content_receivers.get(&section_title).unwrap().recv().unwrap();
            merged_article.push_str(&merged_section);
        }
        drop(_timer);

        let article_body = try!(self.wiki.parse_wikitext(&canonical_title, &merged_article));

        let _marker_timer = Timer::new("Mangled HTML".to_string());
        page.replace_body_and_remove_merge_markers(article_body)
    }

    /// Spawns a single merge thread. The thread starts with `section_content`, accepts (clean
    /// content, candalized content, revision ID) tuples over an MPSC channel, and merges each into
    /// the accumulated content to the extent possible. When the thread receives None over its input
    /// channel, it sends the merged content over another MPSC channel.
    ///
    /// The return value is the tuple (the sender for the input channel, the receiver for the output
    /// channel).
    fn spawn_merge_thread(&self, title: &str, section_title: String, section_content: String) ->
        (Sender<Option<(String, String, u64)>>, Receiver<String>) {
            let (in_sender, in_receiver) = channel::<Option<(String, String, u64)>>();
            let (out_sender, out_receiver) = channel::<String>();
            // TODO: delete
            let section_t = section_title.clone();
            let merger = self.merger.clone();
            let max_consecutive_diff_timeouts = self.max_consecutive_diff_timeouts;
            thread::Builder::new().name(format!("merge-{}-{}", title, section_title)).spawn(move|| {
                let mut merged_content = section_content;
                // As you go backward in time, pages get different enough that they can't be quickly
                // diffed against the current version of the page, and trying to do so is a waste of
                // 500ms per revision. To avoid that, we stop trying to merge after seeing (by
                // default) 3 timeouts in a row.
                let mut consecutive_timeouts = 0;
                let _timer = Timer::new(format!("Merged all revisions of \"{}\"", section_t));
                loop {
                    match in_receiver.recv() {
                        Ok(Some((clean_content, vandalized_content, revision_id))) => {
                            if consecutive_timeouts < max_consecutive_diff_timeouts {
                                let (merge_result, timed_out) = merger.try_merge(
                                    &clean_content, &merged_content, &vandalized_content,
                                    &revision_id.to_string());
                                merged_content = merge_result;
                                if timed_out {
                                    consecutive_timeouts += 1;
                                } else {
                                    consecutive_timeouts = 0;
                                }
                            }
                        },
                        Ok(None) => {
                            out_sender.send(merged_content);
                            drop(_timer);
                            break;
                        },
                        Err(err) => panic!("Failed to receive from in_receiver: {}", err),
                    }
                }
            });
            (in_sender, out_receiver)
        }

    /// Given a list of (section title, section content) pairs, spawns one merge thread for each
    /// section, described in the documentatino on `spawn_merge_thread()`.
    ///
    /// The return value is a 2-tuple of HashMaps. The first maps from the section title to the Sender
    /// for that section's thread's input channel, and the second maps from the section title to the
    /// Receiver for that section's thread's output channel.
    fn spawn_merge_threads<I>(&self, title: &str, sections: I) ->
        (HashMap<String, Sender<Option<(String, String, u64)>>>, HashMap<String, Receiver<String>>)
        where I: IntoIterator<Item=(String, String)> {
            let mut senders_map = HashMap::new();
            let mut receivers_map = HashMap::new();
            for (section_title, section_content) in sections.into_iter() {
                let (in_sender, out_receiver) =
                    self.spawn_merge_thread(title, section_title.clone(), section_content);
                senders_map.insert(section_title.clone(), in_sender);
                receivers_map.insert(section_title, out_receiver);
            }
            (senders_map, receivers_map)
}
}

/// A Wikipedia article can have duplicate section titles (for example, as of this writing,
/// Richard_Feynman has two "Bibliography" sections). This function adds a separator character,
/// followed by "1", "2", "3", etc., to the ends of the duplicate section titles in each (section
/// title, section content) tuple. This makes an iterator suitable for use in building a HashMap,
/// because the keys are all unique. The separator character ensures it's not possible for an input
/// of the form [("t", _), ("t", _), ("t2", _)] to cause still-duplicated section titles in the
/// output.
fn deduplicate_section_titles<I>(mut sections: I) -> Vec<(String, String)>
    where I: IntoIterator<Item=(String, String)> {
    let mut title_counts: HashMap<String, usize> = HashMap::new();
    let mut deduplicated_sections = Vec::new();
    for (section_title, section_content) in sections {
        let entry = title_counts.entry(section_title.clone()).or_insert(0);
        *entry += 1;
        deduplicated_sections.push(
            (section_title + TITLE_COUNT_SEPARATOR + &(*entry).to_string(), section_content));
    }
    deduplicated_sections
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
    let mut diff_size_limit = 1000;
    let mut diff_time_limit_ms = 500;
    let mut max_consecutive_diff_timeouts = 3;
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
        parser.refer(&mut diff_size_limit).add_option(
            &["--diff_size_limit"], Store,
            "The size in bytes at which a diff is considered too big, and is skipped.");
        parser.refer(&mut diff_time_limit_ms).add_option(
            &["--diff_time_limit_ms"], Store,
            "The maximum time (in milliseconds) to attempt to compute a diff before giving up.");
        parser.refer(&mut max_consecutive_diff_timeouts).add_option(
            &["--max_consecutive_diff_timeouts"], Store,
            "The maximum number of consecutive diff-too-large or diff-timeout failures to accept before ceasing to merge a section.");
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
            Client::new(), Merger::new(diff_size_limit, diff_time_limit_ms),
            max_consecutive_diff_timeouts);
    Iron::new(handler).http(("0.0.0.0", port)).unwrap();
}

#[cfg(test)]
mod tests {
    use super::{TITLE_COUNT_SEPARATOR, deduplicate_section_titles};

    #[test]
    fn test_deduplicate_section_titles() {
        let input = vec![("title1".to_owned(), "content1".to_owned()),
                         ("title1".to_owned(), "content2".to_owned()),
                         ("title2".to_owned(), "content3".to_owned()),
                         ("title1".to_owned(), "content4".to_owned())];
        let expected = vec![(format!("title1{}1", TITLE_COUNT_SEPARATOR), "content1".to_owned()),
                            (format!("title1{}2", TITLE_COUNT_SEPARATOR), "content2".to_owned()),
                            (format!("title2{}1", TITLE_COUNT_SEPARATOR), "content3".to_owned()),
                            (format!("title1{}3", TITLE_COUNT_SEPARATOR), "content4".to_owned())];
        assert_eq!(expected, deduplicate_section_titles(input));
    }
}
