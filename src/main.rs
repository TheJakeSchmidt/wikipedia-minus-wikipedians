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
extern crate regex;
extern crate rustc_serialize;
extern crate tempfile;
extern crate tendril;
extern crate url;

mod json;
mod wiki;

use argparse::ArgumentParser;
use argparse::Store;
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
// TODO: there are some places where I've handrolled try!() equivalents. Fix those.
// TODO: make sure I'm returning Results everywhere, and propagating errors correctly. Remove all
// uses of unwrap() that might panic.

// TODO: I'm not so sure these parameter names aren't terrible.
// TODO: would Result make more sense than Option for this return value?
/// Does a 3-way merge, merging `new1` and `new2` under the assumption that both diverged from
/// `old`. Returns None if the strings do not merge together cleanly.
fn merge(old: &str, new1: &str, new2: &str) -> Option<String> {
    fn write_to_temp_file(contents: &str) -> NamedTempFile {
        let tempfile = NamedTempFile::new().unwrap();
        let mut file = OpenOptions::new().write(true).open(tempfile.path()).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        tempfile
    }

    let old_tempfile = write_to_temp_file(old);
    let new1_tempfile = write_to_temp_file(new1);
    let new2_tempfile = write_to_temp_file(new2);

    let mut process = Command::new("diff3");
    process.arg("-m").args(&[new1_tempfile.path(), old_tempfile.path(), new2_tempfile.path()])
        .stdout(Stdio::piped()).stderr(Stdio::null());
    let output = process.output().unwrap();
    if output.status.success() {
        Some(String::from_utf8(output.stdout).unwrap())
    } else {
        None
    }
}

fn replace_node_with_placeholder(original_html: &str, div_id: &str, placeholder: &str)
    -> Result<String, String> {
    // TODO: check errors
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
    // TODO: error handling
    Ok(String::from_utf8(serialized).unwrap())
}

fn find_node_by_id(handle: &Handle, id: &str) -> Result<Handle, String> {
    fn has_matching_id(attributes: &Vec<Attribute>, id: &str) -> bool {
        // TODO: could also do this with filter() and is_empty(). Not sure if that would be better.
        for attribute in attributes {
            // TODO: do I seriously have to construct a StrTendril here?
            // There has to be a better way.
            if attribute.name.local.as_slice() == "id" &&
                attribute.value == tendril::StrTendril::from_str(id).unwrap() {
                    return true;
                }
        }
        return false;
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
        let revisions = try!(self.wiki.get_revisions(title, 60));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    // TODO: this name is terrible.
    fn get_page_with_vandalism_restored(&self, title: &str) -> Result<String, String> {
        let canonical_title = self.wiki.get_canonical_title(title).unwrap();

        let latest_revid = self.wiki.get_latest_revision(&canonical_title).unwrap().revid;
        let mut accumulated_contents =
            self.wiki.get_revision_content(&canonical_title, latest_revid).unwrap();
        let mut revisions = vec![];
        for revision in self.get_vandalism_reversions(&canonical_title).ok().unwrap() {
            let reverted_contents =
                self.wiki.get_revision_content(&canonical_title, revision.revid).unwrap();
            let vandalized_contents =
                self.wiki.get_revision_content(&canonical_title, revision.parentid).unwrap();
            match merge(&reverted_contents, &vandalized_contents, &accumulated_contents) {
                Some(merged_contents) => {
                    info!(
                        concat!("For page \"{}\", restored vandalism ",
                                "https://{}/w/index.php?title={}&diff=prev&oldid={}"),
                        &title, self.wiki.hostname, &canonical_title, revision.revid);
                    accumulated_contents = merged_contents;
                    revisions.push(revision.revid);
                }
                None => (),
            }
        }

        let body = self.wiki.parse_wikitext(&canonical_title, &accumulated_contents).unwrap();
        // Note: "title" rather than "canonical_title", so that redirects look right.
        let current_page_contents = self.wiki.get_current_page_content(&title);
        // TODO: randomize the placeholder string per-request
        let page_contents_with_placeholder =
            replace_node_with_placeholder(
                &current_page_contents, "mw-content-text", "WMW_PLACEHOLDER_TEXT").unwrap();
        Ok(page_contents_with_placeholder.replace("WMW_PLACEHOLDER_TEXT", &body))
    }
}

impl Handler for WikipediaWithoutWikipedians {
    fn handle(&self, request: &mut Request) -> IronResult<Response> {
        if request.url.path.len() == 2 && request.url.path[0] == "wiki" {
            let mut response = Response::with(
                (iron::status::Ok,
                 self.get_page_with_vandalism_restored(&request.url.path[1]).unwrap()));
            response.headers.set(ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
            Ok(response)
        } else {
            // TODO: should I use an HTTP redirect here instead? Would that work? Would it be desirable?
            // TODO: error handling
            let mut wikipedia_response =
                // TODO: not good enough. Needs to include query string. Maybe should be moved to wiki module.
                self.client.get(
                    &format!("https://{}/{}", self.wiki.hostname, request.url.path.join("/")))
                .header(Connection::close())
                .send().unwrap();
            let mut wikipedia_body: Vec<u8> = Vec::new();
            wikipedia_response.read_to_end(&mut wikipedia_body);

            let mut response = Response::with(wikipedia_body);
            response.status = Some(wikipedia_response.status);
            response.headers = wikipedia_response.headers.clone();
            info!("Forwarded request for {} to {}", request.url.path.join("/"),
                  self.wiki.hostname);
            Ok(response)
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
