#![feature(plugin)]
#![plugin(regex_macros)]
#![feature(collections)]

extern crate argparse;
extern crate html5ever;
extern crate html5ever_dom_sink;
extern crate hyper;
extern crate iron;
#[macro_use]
extern crate log;
extern crate log4rs;
extern crate regex;
extern crate rustc_serialize;
extern crate tempfile;
extern crate tendril;
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

mod json;
mod wiki;

use argparse::ArgumentParser;
use argparse::Store;
//use collections::borrow::Borrow;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;

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

/// Does a 3-way merge, merging `new1` and `new2` under the assumption that both diverged from
/// `old`. Returns None if the strings do not merge together cleanly.
fn merge(old: &str, new1: &str, new2: &str) -> Result<Option<String>, String> {
    fn write_to_temp_file(contents: &str) -> Result<NamedTempFile, String> {
        let tempfile = try_display!(NamedTempFile::new(), "Error creating temp file");
        let mut file = try_display!(OpenOptions::new().write(true).open(tempfile.path()),
                                    "Error opening temp file for writing");
        try_display!(file.write_all(contents.as_bytes()), "Error writing to temp file");
        try_display!(file.flush(), "Error flushing temp file");
        Ok(tempfile)
    }

    let old_tempfile = try!(write_to_temp_file(old));
    let new1_tempfile = try!(write_to_temp_file(new1));
    let new2_tempfile = try!(write_to_temp_file(new2));

    let mut process = Command::new("diff3");
    process.arg("-m").args(&[new1_tempfile.path(), old_tempfile.path(), new2_tempfile.path()])
        .stdout(Stdio::piped()).stderr(Stdio::null());
    let output = try_display!(process.output(), "Error getting output from diff3 subprocess");
    if output.status.success() {
        Ok(Some(try_display!(String::from_utf8(output.stdout),
                             "Error converting diff3 output to UTF-8")))
    } else {
        Ok(None)
    }
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
    html5ever::serialize::serialize(&mut serialized, &dom.document, Default::default());
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
        _ => {
            for child in &node.children {
                match find_node_by_id(child, id) {
                    // TODO: this looks weird. Is this an abnormal way to do things?
                    Ok(node) => return Ok(node),
                    _ => continue,
                }
            }
            Err(format!("No node with ID {} found", id))
        },
    }
}

// TODO: rename?
struct WikipediaWithoutWikipedians {
    wiki: Wiki,
    client: Client
}

impl WikipediaWithoutWikipedians {
    fn new(wiki: Wiki, client: Client) -> WikipediaWithoutWikipedians {
        WikipediaWithoutWikipedians { wiki: wiki, client: client }
    }

    /// Returns a vector of Revisions representing all reversions of vandalism for the page `title`.
    fn get_vandalism_reversions(&self, title: &str) -> Result<Vec<Revision>, String> {
        // TODO: Add a flag to control number of revisions, and experiment with it.
        let revisions = try!(self.wiki.get_revisions(title, 60));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    // TODO: this name is terrible.
    fn get_page_with_vandalism_restored(&self, title: &str) -> Result<String, String> {
        let canonical_title = try!(self.wiki.get_canonical_title(title));
        info!("Canonical page title for \"{}\" is \"{}\"", title, canonical_title);

        let latest_revid = try!(self.wiki.get_latest_revision(&canonical_title)).revid;
        let mut accumulated_contents =
            try!(self.wiki.get_revision_content(&canonical_title, latest_revid));
        let mut revisions = vec![];
        for revision in try!(self.get_vandalism_reversions(&canonical_title)) {
            let reverted_contents =
                try!(self.wiki.get_revision_content(&canonical_title, revision.revid));
            let vandalized_contents =
                try!(self.wiki.get_revision_content(&canonical_title, revision.parentid));
            match merge(&reverted_contents, &vandalized_contents, &accumulated_contents) {
                Ok(Some(merged_contents)) => {
                    info!(concat!("For page \"{}\", restored vandalism ",
                                  "https://{}/w/index.php?title={}&diff=prev&oldid={}"),
                          &title, self.wiki.hostname, &canonical_title, revision.revid);
                    accumulated_contents = merged_contents;
                    revisions.push(revision.revid);
                }
                Ok(None) => (),
                Err(msg) => return Err(format!("Error merging revision {} of \"{}\": {}",
                                               revision.revid, title, msg))
            }
        }

        let body = try!(self.wiki.parse_wikitext(&canonical_title, &accumulated_contents));
        // Note: "title" rather than "canonical_title", so that redirects look right.
        let current_page_contents = try!(self.wiki.get_current_page_content(&title));
        // TODO: randomize the placeholder string per-request
        let page_contents_with_placeholder =
            try!(replace_node_with_placeholder(
                &current_page_contents, "mw-content-text", "WMW_PLACEHOLDER_TEXT"));
        Ok(page_contents_with_placeholder.replace("WMW_PLACEHOLDER_TEXT", &body))
    }
}

impl Handler for WikipediaWithoutWikipedians {
    fn handle(&self, request: &mut Request) -> IronResult<Response> {
        if request.url.path.len() == 2 && request.url.path[0] == "wiki" {
            let mut response =
                match self.get_page_with_vandalism_restored(&request.url.path[1]) {
                    Ok(page_contents) => Response::with((iron::status::Ok, page_contents)),
                    // TODO: create an Error type to pass around, so this can distinguish different
                    // types of error (if that would be helpful).
                    // TODO: create a better error page
                    Err(msg) => Response::with(
                        (iron::status::InternalServerError, "<html><body>ERROR</body></html>")),
                };
            response.headers.set(ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
            Ok(response)
        } else {
            // TODO: should I use an HTTP redirect here instead? Would that work? Would it be desirable?
            // TODO: not good enough. Needs to include query string. Maybe should be moved to wiki module.
            match self.client.get(
                &format!("https://{}/{}", self.wiki.hostname, request.url.path.join("/")))
                .header(Connection::close()).send() {
                    Ok(mut wikipedia_response) => {
                        let mut wikipedia_body: Vec<u8> = Vec::new();
                        wikipedia_response.read_to_end(&mut wikipedia_body);

                        let mut response = Response::with(wikipedia_body);
                        response.status = Some(wikipedia_response.status);
                        response.headers = wikipedia_response.headers.clone();
                        info!("Forwarded request for {} to {}", request.url.path.join("/"),
                              self.wiki.hostname);
                        Ok(response)
                    },
                    Err(error) => {
                        let mut response = Response::with(
                            (iron::status::InternalServerError,
                             format!("<html><body>ERROR: {}</body></html>", error)));
                        response.headers.set(ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
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
    {
        let mut parser = ArgumentParser::new();
        parser.set_description("TODO: Usage description");
        parser.refer(&mut port).add_option(&["-p", "--port"], Store, "The port to serve HTTP on.");
        parser.refer(&mut wiki).add_option(&["--wiki"], Store, "The wiki to mirror.");
        parser.parse_args_or_exit();
    }
    let wikipedia_without_wikipedians =
        WikipediaWithoutWikipedians::new(Wiki::new(wiki.to_string(), Client::new()), Client::new());
    Iron::new(wikipedia_without_wikipedians).http(("localhost", port)).unwrap();
}

#[cfg(test)]
mod tests {
    use super::{merge, replace_node_with_placeholder};

    #[test]
    fn test_merge_clean() {
        let old = "First line.\n\nSecond line.\n";
        let new1 = "First line.\n\nSecond line changed.\n";
        let new2 = "First line changed.\n\nSecond line.\n";
        assert_eq!(Some("First line changed.\n\nSecond line changed.\n".to_string()),
                   merge(old, new1, new2).unwrap());
    }

    #[test]
    fn test_merge_conflicting() {
        let old = "First line.\n\nSecond line.\n";
        let new1 = "First line.\n\nSecond line changed one way.\n";
        let new2 = "First line changed.\n\nSecond line changed a different way.\n";
        assert_eq!(None, merge(old, new1, new2).unwrap());
    }

    #[test]
    fn test_merge_special_characters() {
        let old = "First line.\n\nSecond line.\n";
        let new1 = "First line.\n\nSecond line 𐅃.\n";
        let new2 = "First line さようなら.\n\nSecond line.\n";
        assert_eq!(Some("First line さようなら.\n\nSecond line 𐅃.\n".to_string()),
                   merge(old, new1, new2).unwrap());
    }

    #[test]
    fn test_replace_html_content() {
        let original_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\"><p>original text</p></div><div>Other text</div></div></div></body></html>";
        let expected_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\">replaced text</div><div>Other text</div></div></div></body></html>";
        let processed_html = replace_node_with_placeholder(original_html, "mw-content-text", "replaced text").unwrap();
        assert_eq!(expected_html, processed_html);
    }
}
