#![feature(plugin)]
#![plugin(regex_macros)]

extern crate argparse;
extern crate html5ever;
extern crate html5ever_dom_sink;
extern crate hyper;
extern crate iron;
extern crate regex;
extern crate rustc_serialize;
extern crate tempfile;
extern crate tendril;
extern crate url;

mod json;

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
use rustc_serialize::json::Json;
use tempfile::NamedTempFile;
use url::percent_encoding;

use json::JsonPathElement::{Key, Only};

// TODO: consider doing s/en.wikipedia.org/this app's url/ on the HTML before serving it. This
// currently works fine, but might not over HTTPS.
// TODO: there are some places where I've handrolled try!() equivalents. Fix those.
// TODO: make sure I'm returning Results everywhere, and propagating errors correctly. Remove all
// uses of unwrap() that might panic.
// TODO: The page "Battle_of_Palo_Alto" is truncated. Figure out the best way to debug.

// TODO: move all the Wikipedia code to a separate module. If possible, remove json dependency from
// this module entirely.
struct Wiki {
    hostname: String,
}

struct Revision {
    revid: u64,
    parentid: u64,
    comment: String,
}

impl Wiki {
    /// Constructs a Wiki object representing the wiki at `hostname` (e.g. "en.wikipedia.org").
    pub fn new(hostname: &str) -> Wiki {
        Wiki { hostname: hostname.to_string() }
    }

    // TODO: return a Result
    /// Calls the MediaWiki API with the given parameters and format=json. Returns the raw JSON.
    fn call_mediawiki_api(&self, parameters: Vec<(&str, &str)>) -> String {
        let post_body =
            parameters.into_iter().map(|p| format!("{}={}", p.0, p.1))
            .collect::<Vec<_>>().join("&") + "&format=json";

        let client = Client::new();
        let mut response = client.post(&format!("https://{}/w/api.php", self.hostname))
            .body(&post_body)
            .header(Connection::close())
            .send().unwrap();

        let mut body = String::new();
        response.read_to_string(&mut body).unwrap();
        body
    }

    /// Returns the last `limit` revisions for the page `title`.
    pub fn get_revision_ids(&self, title: &str, limit: u64) -> Result<Vec<Revision>, String> {
        let json_str = self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title),
                 ("rvprop", "comment|ids"), ("rvlimit", &limit.to_string())]);
        let json = Json::from_str(&json_str).unwrap();
        let revisions = try!(
            json::get_json_array(&json, &[Key("query"), Key("pages"), Only, Key("revisions")]));

        // Check that  all revisions have revid and parentid (they all should).
        // TODO: what about making a Vec<Result<Revision>, String> and then rolling that up with a fold()? Would eliminate the repetition of the get_json_* calls.
        if revisions.into_iter().any(|revision| {
            json::get_json_number(revision, &[Key("revid")]).is_err() ||
            json::get_json_number(revision, &[Key("parentid")]).is_err() ||
            json::get_json_string(revision, &[Key("comment")]).is_err()
        }) {
            Err(format!(
                "One or more revisions of page \"{}\" have missing or invalid field", title))
        } else {
            Ok(revisions.into_iter().map(|revision| {
                Revision {
                    revid: json::get_json_number(revision, &[Key("revid")]).unwrap(),
                    parentid: json::get_json_number(revision, &[Key("parentid")]).unwrap(),
                    comment: json::get_json_string(revision, &[Key("comment")]).unwrap().to_string()
                }
            }).collect())
        }
    }

    /// Returns the latest revision ID for the page `title`.
    pub fn get_latest_revision_id(&self, title: &str) -> Result<u64, String> {
        // TODO: Can this be a one-liner? Does try!() work properly like that?
        let revisions = try!(self.get_revision_ids(title, 1));
        Ok(revisions[0].revid)
    }

    /// Returns the contents of the page `title` as of (i.e., immediately after) revision `id`.
    pub fn get_revision_content(&self, title: &str, id: u64) -> Result<String, String> {
        let json_str = self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title), ("rvprop", "content"),
                 ("rvlimit", "1"), ("rvstartid", &id.to_string())]);
        let json = Json::from_str(&json_str).unwrap();
        match json::get_json_string(
            &json, &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("*")]) {
            Ok(content) => Ok(content.to_string()),
            Err(msg) => Err(msg),
        }
    }

    /// Follows all redirects to find the canonical name of the page at `title`.
    pub fn get_canonical_title(&self, title: &str) -> Result<String, String> {
        let latest_revision_id = self.get_latest_revision_id(title).unwrap();
        let page_contents = self.get_revision_content(title, latest_revision_id).unwrap();

        let regex = regex!(r"#REDIRECT \[\[([^]]+)\]\].*");
        match regex.captures(&page_contents) {
            Some(captures) => self.get_canonical_title(captures.at(1).unwrap()),
            None => {
                println!("Canonical page title is \"{}\"", title);
                Ok(title.to_string())
            },
        }
    }

    /// Parses the wikitext in `wikitext` as though it were the contents of the page `title`,
    /// returning the rendered HTML.
    pub fn parse_wikitext(&self, title: &str, wikitext: &str) -> Result<String, String> {
        let encoded_wikitext =
            percent_encoding::percent_encode(wikitext.as_bytes(), percent_encoding::QUERY_ENCODE_SET);
        let html = self.call_mediawiki_api(
            vec![("action", "parse"), ("prop", "text"), ("disablepp", ""), ("contentmodel", "wikitext"),
                 ("title", title), ("text", &encoded_wikitext)]);
        // TODO: check return value
        let json = Json::from_str(&html).unwrap();
        match json::get_json_string(&json, &[Key("parse"), Key("text"), Key("*")]) {
            Ok(contents) => Ok(contents.to_string()),
            Err(message) => Err(message),
        }
    }

    /// Gets the current, fully-rendered (**HTML**) contents of the page `title`.
    pub fn get_current_page_content(title: &str) -> String {
        // TODO: should this struct be keeping a Client instead of creating new ones for each method
        // call, here and in call_mediawiki_api?
        let client = Client::new();
        let mut res = client.get(
            &format!("https://en.wikipedia.org/wiki/{}", title))
            .header(Connection::close())
            .send().unwrap();

        let mut body = String::new();
        res.read_to_string(&mut body).unwrap();

        body
    }
}

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
}

impl WikipediaWithoutWikipedians {
    fn new(wiki: Wiki) -> WikipediaWithoutWikipedians {
        WikipediaWithoutWikipedians { wiki: wiki }
    }

    /// Returns a vector of Revisions representing all reversions of vandalism for the page `title`.
    fn get_vandalism_reversions(&self, title: &str) -> Result<Vec<Revision>, String> {
        let revisions = try!(self.wiki.get_revision_ids(title, 60));
        Ok(revisions.into_iter().filter(|revision| revision.comment.contains("vandal")).collect())
    }

    // TODO: this name is terrible.
    fn get_page_with_vandalism_restored(&self, title: &str) -> Result<String, String> {
        let canonical_title = self.wiki.get_canonical_title(title).unwrap();

        let latest_revid = self.wiki.get_latest_revision_id(&canonical_title).unwrap();
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
                    // TODO: replace this with a log statement, if there's a good logging framework.
                    println!(
                        "For page \"{}\", restored vandalism https://en.wikipedia.org/w/index.php?title={}&diff=prev&oldid={}",
                        &title, &canonical_title, revision.revid);
                    accumulated_contents = merged_contents;
                    revisions.push(revision.revid);
                }
                None => (),
            }
        }

        // TODO: replace this with a log statement, if there's a good logging framework.
        println!("For page \"{}\", restored vandalisms reverted in: {:?}", &title, revisions);

        let body = self.wiki.parse_wikitext(title, &accumulated_contents).unwrap();
        // Note: "title" rather than "canonical_title", so that redirects look right.
        let current_page_contents = Wiki::get_current_page_content(&title);
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
            let client = Client::new();
            // TODO: error handling
            let mut wikipedia_response =
                // TODO: not good enough. Needs to include query string. Needs to use hostname instead of always en.wikipedia.org. Maybe should be moved to Wiki.
                client.get(&format!("https://en.wikipedia.org/{}", request.url.path.join("/")))
                .header(Connection::close())
                .send().unwrap();
            let mut wikipedia_body: Vec<u8> = Vec::new();
            wikipedia_response.read_to_end(&mut wikipedia_body);

            let mut response = Response::with(wikipedia_body);
            response.status = Some(wikipedia_response.status);
            response.headers = wikipedia_response.headers.clone();
            println!("Forwarded request for {} to en.wikipedia.org", request.url.path.join("/"));
            Ok(response)
        }
    }
}

fn main() {
    let mut port = 3000;
    {
        let mut parser = ArgumentParser::new();
        parser.set_description("TODO: Usage description");
        parser.refer(&mut port).add_option(&["-p", "--port"], Store, "The port to serve HTTP on.");
        parser.parse_args_or_exit();
    }
    let wikipedia_without_wikipedians =
        WikipediaWithoutWikipedians::new(Wiki::new("en.wikipedia.org"));
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
        assert_eq!("First line changed.\n\nSecond line changed.\n", merge(old, new1, new2).unwrap());
    }

    #[test]
    fn test_merge_conflicting() {
        let old = "First line.\n\nSecond line.\n";
        let new1 = "First line.\n\nSecond line changed one way.\n";
        let new2 = "First line changed.\n\nSecond line changed a different way.\n";
        assert_eq!(None, merge(old, new1, new2));
    }

    #[test]
    fn test_merge_special_characters() {
        let old = "First line.\n\nSecond line.\n";
        let new1 = "First line.\n\nSecond line 𐅃.\n";
        let new2 = "First line さようなら.\n\nSecond line.\n";
        assert_eq!("First line さようなら.\n\nSecond line 𐅃.\n", merge(old, new1, new2).unwrap());
    }

    #[test]
    fn test_replace_html_content() {
        let original_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\"><p>original text</p></div><div>Other text</div></div></div></body></html>";
        let expected_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\">replaced text</div><div>Other text</div></div></div></body></html>";
        let processed_html = replace_node_with_placeholder(original_html, "mw-content-text", "replaced text").unwrap();
        assert_eq!(expected_html, processed_html);
    }
}
