extern crate html5ever;
extern crate html5ever_dom_sink;
extern crate hyper;
extern crate rustc_serialize;
extern crate tempfile;
extern crate tendril;
extern crate url;

mod json;

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
use rustc_serialize::json::Json;
use tempfile::NamedTempFile;
use url::percent_encoding;

use json::JsonPathElement::{Key, Only};

// TODO: there are some places where I've handrolled try!() equivalents. Fix those.
// TODO: make sure I'm returning Results everywhere, and propagating errors correctly.

// TODO: Can I get rid of the repeated "Zachary_Taylor"s everywhere? Surely the MediaWiki API doesn't actually need that - I can't imagine revision IDs aren't unique across all pages.

fn get_revisions(page: &str, limit: i32) -> String {
    let client = Client::new();
    let mut res = client.get(
        &format!(
            "https://en.wikipedia.org/w/api.php?action=query&prop=revisions&titles={}&rvprop=timestamp|user|comment|ids&rvlimit={}&format=json",
            page, limit))
        .header(Connection::close())
        .send().unwrap();

    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();

    body
}

// TODO: this name is terrible.
// Returns pairs of (revid, parentid)
fn get_revert_revision_ids(page: &str) -> Result<Vec<(u64, u64)>, String> {
    let json = Json::from_str(&get_revisions(page, 60)).unwrap();
    let revisions = try!(
        json::get_json_array(&json, &[Key("query"), Key("pages"), Only, Key("revisions")]));

    // Filter for revisions that mention vandalism
    let filtered_revisions =
        revisions.into_iter().filter(|revision| {
            match json::get_json_string(revision, &[Key("comment")]) {
                Ok(ref comment) => comment.contains("vandal"),
                // TODO: need to warn somehow when this happens, because it generally shouldn't.
                _ => false,
            }
        })
        // Filter out revisions with missing revid or parentid (which shouldn't happen).
        .filter(|revision| {
            match (json::get_json_number(revision, &[Key("revid")]),
                   json::get_json_number(revision, &[Key("parentid")])) {
                (Ok(_), Ok(_)) => true,
                // TODO: need to warn somehow when this happens, because it should never.
                _ => false,
            }});
    Ok(filtered_revisions.map(|revision| {
        (json::get_json_number(revision, &[Key("revid")]).unwrap(),
         json::get_json_number(revision, &[Key("parentid")]).unwrap())
    }).collect())
}

fn get_latest_revision_id(page: &str) -> u64 {
    let json = Json::from_str(&get_revisions(page, 1)).unwrap();
    let revision_id = json::get_json_number(
        &json, &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("revid")]);
    match revision_id {
        Ok(revision_id) => revision_id,
        // TODO: This function should return a Result so we don't have to panic here.
        Err(message) => panic!(format!("Failed to get latest revision ID: \"{}\"", message)),
    }
}

fn get_revision(page: &str, id: u64) -> String {
    let client = Client::new();
    let mut res = client.get(
        &format!(
            "https://en.wikipedia.org/w/api.php?action=query&prop=revisions&titles={}&rvprop=content&rvlimit=1&rvstartid={}&format=json",
            page, id))
        .header(Connection::close())
        .send().unwrap();

    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();
    body
}

fn get_revision_content(page: &str, id: u64) -> String {
    // TODO: this function should return a Result so we don't have to unwrap() here.
    json::get_json_string(
        &Json::from_str(&get_revision(page, id)).unwrap(),
        &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("*")]).unwrap().to_string()
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

fn render(wikitext: &str) -> Result<String, String> {
    let post_body = format!(
        "text={}", percent_encoding::percent_encode(
            wikitext.as_bytes(), percent_encoding::QUERY_ENCODE_SET));

    let client = Client::new();
    let mut res = client.post(
        "https://en.wikipedia.org/w/api.php?action=parse&format=json&prop=text&disablepp=&contentmodel=wikitext")
        .body(&post_body)
        .header(Connection::close())
        .send().unwrap();

    let mut body = String::new();
    res.read_to_string(&mut body).unwrap();

    println!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    println!("{}", body);
    // TODO: check return value
    let json = Json::from_str(&body).unwrap();
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

fn process_html(original_html: &str, div_id: &str, replacement_text: &str) -> Result<String, String> {
    // TODO: check errors
    let html = tendril::StrTendril::from_str(original_html).unwrap();
    let mut dom: RcDom = html5ever::parse(html5ever::one_input(html), Default::default());

    let handle = try!(find_node_by_id(&dom.get_document(), "mw-content-text"));
    let child_handles =
        (&handle.borrow().children).into_iter().map(|child| child.clone()).collect::<Vec<_>>();
    for child_handle in child_handles {
        dom.remove_from_parent(child_handle);
    }
    dom.append(handle,
               html5ever::tree_builder::interface::NodeOrText::AppendText(
                   tendril::StrTendril::from_str(replacement_text).unwrap()));
    let mut serialized: Vec<u8> = vec![];
    html5ever::serialize::serialize(&mut serialized, &dom.document, Default::default());
    // TODO: error handling
    Ok(String::from_utf8(serialized).unwrap())
}

fn main() {
    let latest_revid = get_latest_revision_id("Zachary_Taylor");
    let mut accumulated_contents = get_revision_content("Zachary_Taylor", latest_revid);
    let mut revisions = vec![];
    for (revert_revid, vandalism_revid) in get_revert_revision_ids("Zachary_Taylor").ok().unwrap() {
        let reverted_contents = get_revision_content("Zachary_Taylor", revert_revid);
        let vandalized_contents = get_revision_content("Zachary_Taylor", vandalism_revid);
        match merge(&reverted_contents, &vandalized_contents, &accumulated_contents) {
            Some(merged_contents) => {
                accumulated_contents = merged_contents;
                revisions.push(revert_revid);
            }
            None => (),
        }
    }

    match render(&accumulated_contents) {
        Ok(contents) => {
            println!("Restored vandalisms reverted in: {:?}", revisions);
            println!("{}", contents);
        },
        Err(message) => panic!(message),
    }
}

#[cfg(test)]
mod tests {
    use super::{merge, process_html};

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
        let processed_html = process_html(original_html, "mw-content-text", "replaced text").unwrap();
        assert_eq!(expected_html, processed_html);
    }
}
