extern crate hyper;
extern crate rustc_serialize;
extern crate tempfile;

use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;

use hyper::Client;
use hyper::header::Connection;
use rustc_serialize::json::Json;
use rustc_serialize::json::Json::{Array, Object};
use tempfile::NamedTempFile;

// TODO: there are some places where I've handrolled try!() equivalents. Fix those.
// TODO: make sure I'm returning Results everywhere, and propagating errors correctly.

// TODO: Can I get rid of the repeated "Zachary_Taylor"s everywhere? Surely the MediaWiki API doesn't actually need that - I can't imagine revision IDs aren't unique across all pages.

enum JsonPathElement {
    Key(&'static str), // Use the value associated with a specific key.
    Only,              // Use the only value in an array or object.
}

use JsonPathElement::Key;
use JsonPathElement::Only;

// TODO: return &str with a lifetime matching path_elements
fn pretty_print(path_elements: &[JsonPathElement]) -> String {
    // TODO: add (root) to the beginning always.
    if path_elements.is_empty() {
        "(root)".to_string()
    } else {
        path_elements.into_iter().map(|path_element| {
            match path_element {
                // TODO: why do I need the JsonPathElement:: here? I think I don't.
                &JsonPathElement::Key(ref key) => key.to_string(), // TODO: can I remove to_string()?
                &JsonPathElement::Only => "(only)".to_string(),
            }
        }).collect::<Vec<_>>().join(".")
    }
}

// TODO: document
fn get_json_value<'a>(json: &'a Json, path: &[JsonPathElement], index: usize) ->
    Result<&'a Json, String> {
    if index == path.len() {
        return Ok(json);
    }
    match path[index] {
        JsonPathElement::Key(key) => {
            match json {
                &Object(ref obj) => {
                    match obj.get(key) {
                        Some(value) => get_json_value(value, path, index + 1),
                        None => Err(format!("Key {} not found in {}", key, pretty_print(&path[0 .. index]))),
                    }
                }
                _ => Err(format!("Asked for key {} in {}, but value is not an object",
                         key, pretty_print(&path[0 .. index]))),
            }
        },
        JsonPathElement::Only => {
            match json {
                &Object(ref obj) =>
                    if obj.len() == 1 {
                        get_json_value(obj.values().next().unwrap(), path, index + 1)
                    } else {
                        Err(format!("Asked for only key in {}, but object has {} values",
                                    pretty_print(&path[0 .. index]), obj.len()))
                    },
                &Array(ref vec) =>
                    if vec.len() == 1 {
                        get_json_value(vec.first().unwrap(), path, index + 1)
                    } else {
                        Err(format!("Asked for only key in {}, but array has {} elements",
                                    pretty_print(&path[0 .. index]), vec.len()))
                    },
                _ => Err(format!("Asked for only key in {}, but value is not an object or array",
                                 pretty_print(&path[0 .. index]))),
            }
        },
    }
}

fn get_json_array<'a>(json: &'a Json, path: &[JsonPathElement]) -> Result<&'a Vec<Json>, String> {
    match get_json_value(json, path, 0) {
        Ok(&Json::Array(ref value)) => Ok(value),
        Ok(..) => Err(format!("Asked for array {}, but value is not an array",
                              pretty_print(&path[..]))),
        Err(message) => Err(message),
    }
}

fn get_json_number(json: &Json, path: &[JsonPathElement]) -> Result<u64, String> {
    match get_json_value(json, path, 0) {
        Ok(ref value) => {
            if value.is_number() {
                Ok(value.as_u64().unwrap())
            } else {
                Err(format!("Asked for number {}, but value is not a number",
                            pretty_print(&path[..])))
            }
        },
        Err(message) => Err(message),
    }
}

fn get_json_string<'a>(json: &'a Json, path: &[JsonPathElement]) -> Result<&'a str, String> {
    match get_json_value(json, path, 0) {
        Ok(&Json::String(ref value)) => Ok(value),
        Ok(..) => Err(format!("Asked for string {}, but value is not a string",
                              pretty_print(&path[..]))),
        Err(message) => Err(message),
    }
}

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
    let revisions = try!(get_json_array(&json, &[Key("query"), Key("pages"), Only, Key("revisions")]));

    // Filter for revisions that mention vandalism
    let filtered_revisions =
        revisions.into_iter().filter(|revision| {
            match get_json_string(revision, &[Key("comment")]) {
                Ok(ref comment) => comment.contains("vandal"),
                // TODO: need to warn somehow when this happens, because it generally shouldn't.
                _ => false,
            }
        })
        // Filter out revisions with missing revid or parentid (which shouldn't happen).
        .filter(|revision| {
            match (get_json_number(revision, &[Key("revid")]),
                   get_json_number(revision, &[Key("parentid")])) {
                (Ok(_), Ok(_)) => true,
                // TODO: need to warn somehow when this happens, because it should never.
                _ => false,
            }});
    Ok(filtered_revisions.map(|revision| {
        (get_json_number(revision, &[Key("revid")]).unwrap(),
         get_json_number(revision, &[Key("parentid")]).unwrap())
    }).collect())
}

fn get_latest_revision_id(page: &str) -> u64 {
    let json = Json::from_str(&get_revisions(page, 1)).unwrap();
    let revision_id = get_json_number(
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
    get_json_string(
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
    use super::{get_json_array, get_json_string, get_json_number, merge};
    use super::JsonPathElement::*;
    use rustc_serialize::json::Json;

    #[test]
    fn test_get_json_value_key() {
        assert_eq!(Ok("val"),
                   get_json_string(&Json::from_str("{\"key\": \"val\"}").unwrap(),
                                  &[Key("key")]))
    }

    #[test]
    fn test_get_json_value_only_with_object() {
        let json = Json::from_str("{\"key\": \"val\"}").unwrap();
        let value = get_json_string(&json, &[Only]);
        assert_eq!(Ok("val"), value);
    }

    #[test]
    fn test_get_json_value_only_with_array() {
        let json = Json::from_str("[\"val1\"]").unwrap();
        let value = get_json_string(&json, &[Only]);
        assert_eq!(Ok("val1"), value);
    }

    #[test]
    fn test_get_json_value_multiple_elements() {
        assert_eq!(
            Ok("val3"),
            get_json_string(
                &Json::from_str(
                    "{\"key1\": \"val1\", \"key2\": [{\"key3\": \"val3\"}]}").unwrap(),
                &[Key("key2"), Only, Only]))
    }

    fn assert_error_message<T>(result: &Result<T, String>, expected_message: &str) {
        match result {
            &Ok(..) => panic!(format!("Expected error message: \"{}\"", expected_message)),
            &Err(ref message) if message == expected_message => return,
            &Err(ref message) => panic!(format!("Wrong error message: \"{}\"", message)),
        }
    }

    #[test]
    fn test_get_json_value_key_not_found() {
        assert_error_message(
            &get_json_string(
                &Json::from_str("{\"key\": \"val\"}").unwrap(), &[Key("wrong_key")]),
            "Key wrong_key not found in (root)");
    }

    #[test]
    fn test_get_json_value_key_not_object() {
        for json in &["{\"key1\": 4}",
                      "{\"key1\": false}",
                      "{\"key1\": \"val1\"}",
                      "{\"key1\": [1, 2, 3]}",
                      "{\"key1\": null}"] {
            assert_error_message(
                &get_json_string(&Json::from_str(json).unwrap(), &[Key("key1"), Key("key2")]),
                "Asked for key key2 in key1, but value is not an object");
        }
    }

    #[test]
    fn test_get_json_value_only_object_empty() {
        assert_error_message(
            &get_json_string(
                &Json::from_str("{\"key\": {}}").unwrap(), &[Key("key"), Only]),
            "Asked for only key in key, but object has 0 values");
    }

    #[test]
    fn test_get_json_value_only_object_multiple_values() {
        assert_error_message(
            &get_json_string(
                &Json::from_str("{\"key\": {\"key1\": \"val1\", \"key2\": \"val2\"}}").unwrap(),
                &[Key("key"), Only]),
            "Asked for only key in key, but object has 2 values");
    }
 
    #[test]
    fn test_get_json_value_only_array_empty() {
        assert_error_message(
            &get_json_string(
                &Json::from_str("{\"key\": []}").unwrap(), &[Key("key"), Only]),
            "Asked for only key in key, but array has 0 elements");
    }

    #[test]
    fn test_get_json_value_only_array_multiple_values() {
        assert_error_message(
            &get_json_string(
                &Json::from_str("{\"key\": [\"val1\", \"val2\"]}").unwrap(),
                &[Key("key"), Only]),
            "Asked for only key in key, but array has 2 elements");
    }

    #[test]
    fn test_get_json_value_only_not_object_or_array() {
        for json in &["{\"key1\": 4}",
                      "{\"key1\": false}",
                      "{\"key1\": \"val1\"}",
                      "{\"key1\": null}"] {
            assert_error_message(
                &get_json_string(&Json::from_str(json).unwrap(), &[Key("key1"), Only]),
                "Asked for only key in key1, but value is not an object or array");
        }
    }

    #[test]
    fn test_get_json_array_wrong_type() {
        for json in &["{\"key1\": 4}",
                      "{\"key1\": \"val1\"}",
                      "{\"key1\": false}",
                      "{\"key1\": {\"key2\": \"val1\"}}",
                      "{\"key1\": null}"] {
            assert_error_message(
                &get_json_array(&Json::from_str(json).unwrap(), &[Key("key1")]),
                "Asked for array key1, but value is not an array");
        }
    }

    #[test]
    fn test_get_json_number_wrong_type() {
        for json in &["{\"key1\": \"val1\"}",
                      "{\"key1\": false}",
                      "{\"key1\": [\"val1\"]}",
                      "{\"key1\": {\"key2\": \"val1\"}}",
                      "{\"key1\": null}"] {
            assert_error_message(
                &get_json_number(&Json::from_str(json).unwrap(), &[Key("key1")]),
                "Asked for number key1, but value is not a number");
        }
    }

    #[test]
    fn test_get_json_string_wrong_type() {
        for json in &["{\"key1\": 4}",
                      "{\"key1\": false}",
                      "{\"key1\": [\"val1\"]}",
                      "{\"key1\": {\"key2\": \"val1\"}}",
                      "{\"key1\": null}"] {
            assert_error_message(
                &get_json_string(&Json::from_str(json).unwrap(), &[Key("key1")]),
                "Asked for string key1, but value is not a string");
        }
    }

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
