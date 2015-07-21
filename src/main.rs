extern crate hyper;
extern crate rustc_serialize;
extern crate tempfile;

mod json;

use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;

use hyper::Client;
use hyper::header::Connection;
use rustc_serialize::json::Json;
use tempfile::NamedTempFile;

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

fn main() {
    let latest_revid = get_latest_revision_id("Zachary_Taylor");
    // TODO: this is disgusting.
    let revision_ids = get_revert_revision_ids("Zachary_Taylor").ok().unwrap();
    let processed_contents = revision_ids.into_iter().fold(
        (get_revision_content("Zachary_Taylor", latest_revid), vec![]),
        |accumulated_contents, revision_ids| {
            let revert_revid = revision_ids.0;
            let vandalism_revid = revision_ids.1;
            match merge(&get_revision_content("Zachary_Taylor", revert_revid),
                        &get_revision_content("Zachary_Taylor", vandalism_revid),
                        &accumulated_contents.0) {
                Some(merged_contents) => {
                    let mut merged_revision_ids = accumulated_contents.1;
                    merged_revision_ids.push(revert_revid);
                    (merged_contents, merged_revision_ids)
                }
                None => accumulated_contents
            }
        });

    println!("Restored vandalisms reverted in: {:?}", processed_contents.1);
    println!("{}", processed_contents.0);
}

#[cfg(test)]
mod tests {
    use super::{merge};

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
}
