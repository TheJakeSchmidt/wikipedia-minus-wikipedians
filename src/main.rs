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
use iron::mime::Mime;
use iron::mime::SubLevel;
use iron::mime::TopLevel;
use rustc_serialize::json::Json;
use tempfile::NamedTempFile;
use url::percent_encoding;

use json::JsonPathElement::{Key, Only};

// TODO: consider doing s/en.wikipedia.org/this app's url/ on the HTML before serving it. This
// currently works fine, but might not over HTTPS.
// TODO: Do nested functions work? If so, use those where appropriate.
// TODO: there are some places where I've handrolled try!() equivalents. Fix those.
// TODO: make sure I'm returning Results everywhere, and propagating errors correctly. Remove all
// uses of unwrap() that might panic.
// TODO: The page "Battle_of_Palo_Alto" is truncated. Figure out the best way to debug.

// TODO: return a Result
fn call_wikimedia_api(parameters: Vec<(&str, &str)>) -> String {
    let post_body =
        parameters.into_iter().map(|p| format!("{}={}", p.0, p.1))
        .collect::<Vec<_>>().join("&") + "&format=json";

    let client = Client::new();
    let mut response = client.post("https://en.wikipedia.org/w/api.php")
        .body(&post_body)
        .header(Connection::close())
        .send().unwrap();

    let mut body = String::new();
    response.read_to_string(&mut body).unwrap();
    body
}

// TODO: this name is terrible.
// Returns pairs of (revid, parentid)
fn get_revert_revision_ids(title: &str) -> Result<Vec<(u64, u64)>, String> {
    let json_str = call_wikimedia_api(
        vec![("action", "query"), ("prop", "revisions"), ("titles", title),
             ("rvprop", "comment|ids"), ("rvlimit", "60")]);

    let json = Json::from_str(&json_str).unwrap();
    let revisions = try!(
        json::get_json_array(&json, &[Key("query"), Key("pages"), Only, Key("revisions")]));

    // Filter for revisions that mention vandalism, and that have revid and parentid (which they all
    // should, but merits checking).
    Ok(revisions.into_iter().filter(|revision| {
        match (json::get_json_string(revision, &[Key("comment")]),
               json::get_json_number(revision, &[Key("revid")]),
               json::get_json_number(revision, &[Key("parentid")])) {
            (Ok(ref comment), Ok(_), Ok(_)) => comment.contains("vandal"),
            _ => false,
        }})
       .map(|revision| {
           (json::get_json_number(revision, &[Key("revid")]).unwrap(),
            json::get_json_number(revision, &[Key("parentid")]).unwrap())
       }).collect())
}

fn get_latest_revision_id(title: &str) -> Result<u64, String> {
    let json_str = call_wikimedia_api(
        vec![("action", "query"), ("prop", "revisions"), ("titles", title), ("rvprop", "ids"),
             ("rvlimit", "1")]);
    let json = Json::from_str(&json_str).unwrap();
    json::get_json_number(
        &json, &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("revid")])
}

fn get_revision_content(title: &str, id: u64) -> Result<String, String> {
    let json_str = call_wikimedia_api(
        vec![("action", "query"), ("prop", "revisions"), ("titles", title), ("rvprop", "content"),
             ("rvlimit", "1"), ("rvstartid", &id.to_string())]);
    let json = Json::from_str(&json_str).unwrap();
    match json::get_json_string(
        &json, &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("*")]) {
        Ok(content) => Ok(content.to_string()),
        Err(msg) => Err(msg),
    }
}

fn get_canonical_title(title: &str) -> Result<String, String> {
    let latest_revision_id = get_latest_revision_id(title).unwrap();
    let page_contents = get_revision_content(title, latest_revision_id).unwrap();

    let regex = regex!(r"#REDIRECT \[\[([^]]+)\]\].*");
    match regex.captures(&page_contents) {
        Some(captures) => get_canonical_title(captures.at(1).unwrap()),
        None => {
            println!("Canonical page title is \"{}\"", title);
            Ok(title.to_string())
        },
    }
}

fn write_to_temp_file(contents: &str) -> NamedTempFile {
    let tempfile = NamedTempFile::new().unwrap();
    let mut file = OpenOptions::new().write(true).open(tempfile.path()).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    file.flush().unwrap();
    tempfile
}

// TODO: I'm not so sure these parameter names aren't terrible.
fn merge(old: &str, new1: &str, new2: &str) -> Option<String> {
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

fn render(title: &str, wikitext: &str) -> Result<String, String> {
    let encoded_wikitext =
        percent_encoding::percent_encode(wikitext.as_bytes(), percent_encoding::QUERY_ENCODE_SET);
    let html = call_wikimedia_api(
        vec![("action", "parse"), ("prop", "text"), ("disablepp", ""), ("contentmodel", "wikitext"),
             ("title", title), ("text", &encoded_wikitext)]);
    // TODO: check return value
    let json = Json::from_str(&html).unwrap();
    match json::get_json_string(&json, &[Key("parse"), Key("text"), Key("*")]) {
        Ok(contents) => Ok(contents.to_string()),
        Err(message) => Err(message),
    }
}

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

fn find_node_by_id(handle: &Handle, id: &str) -> Result<Handle, String> {
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

fn replace_node_with_placeholder(original_html: &str, div_id: &str, placeholder: &str) -> Result<String, String> {
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

fn get_current_page(title: &str) -> String {
    let client = Client::new();
    let mut res = client.get(
        &format!("https://en.wikipedia.org/wiki/{}", title))
        .header(Connection::close())
        .send().unwrap();

    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();

    body
}

// TODO: this name is terrible.
fn get_page_with_vandalism_restored(title: &str) -> Result<String, String> {
    let canonicalized_title = get_canonical_title(title).unwrap();

    let latest_revid = get_latest_revision_id(&canonicalized_title).unwrap();
    let mut accumulated_contents = get_revision_content(&canonicalized_title, latest_revid).unwrap();
    let mut revisions = vec![];
    for (revert_revid, vandalism_revid) in get_revert_revision_ids(&canonicalized_title).ok().unwrap() {
        let reverted_contents = get_revision_content(&canonicalized_title, revert_revid).unwrap();
        let vandalized_contents = get_revision_content(&canonicalized_title, vandalism_revid).unwrap();
        match merge(&reverted_contents, &vandalized_contents, &accumulated_contents) {
            Some(merged_contents) => {
                // TODO: replace this with a log statement, if there's a good logging framework.
                println!(
                    "For page \"{}\", restored vandalism https://en.wikipedia.org/w/index.php?title={}&diff=prev&oldid={}",
                         &title, &canonicalized_title, revert_revid);
                accumulated_contents = merged_contents;
                revisions.push(revert_revid);
            }
            None => (),
        }
    }

    // TODO: replace this with a log statement, if there's a good logging framework.
    println!("For page \"{}\", restored vandalisms reverted in: {:?}", &title, revisions);

    let rendered_body = render(title, &accumulated_contents).unwrap();
    // Note: "title" rather than "canonicalized_title", so that redirects look right.
    let current_page_contents = get_current_page(&title);
    // TODO: randomize the placeholder string per-request
    let page_contents_with_placeholder =
        replace_node_with_placeholder(
            &current_page_contents, "mw-content-text", "WMW_PLACEHOLDER_TEXT").unwrap();
    Ok(page_contents_with_placeholder.replace("WMW_PLACEHOLDER_TEXT", &rendered_body))
}

fn serve_request(request: &mut Request) -> IronResult<Response> {
    if request.url.path.len() == 2 && request.url.path[0] == "wiki" {
        let mut response = Response::with(
            (iron::status::Ok, get_page_with_vandalism_restored(&request.url.path[1]).unwrap()));
        response.headers.set(ContentType(Mime(TopLevel::Text, SubLevel::Html, vec![])));
        Ok(response)
    } else {
        let client = Client::new();
        // TODO: error handling
        let mut wikipedia_response =
            // TODO: not good enough. Needs to include query string.
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

fn main() {
    let mut port = 3000;
    {
        let mut parser = ArgumentParser::new();
        parser.set_description("TODO: Usage description");
        parser.refer(&mut port).add_option(&["-p", "--port"], Store, "The port to serve HTTP on.");
        parser.parse_args_or_exit();
    }
    Iron::new(serve_request).http(("localhost", port)).unwrap();
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
