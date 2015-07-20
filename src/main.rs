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
use rustc_serialize::json;
use tempfile::NamedTempFile;

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

fn get_vandalism_revision_ids(page: &str) -> Vec<u64> {
    let json = json::Json::from_str(&get_revisions(page, 50)).unwrap();

    let pages = json.as_object().unwrap().get("query").unwrap().as_object().unwrap().get("pages").unwrap().as_object().unwrap();
    let key = pages.keys().next().unwrap();
    pages.get(key).unwrap().as_object().unwrap()
        .get("revisions").unwrap().as_array().unwrap().into_iter()
        .map(|revision| revision.as_object().unwrap())
        .filter(|revision| { revision.get("comment").unwrap().as_string().unwrap().contains("vandal") })
        .map(|revision| revision.get("parentid").unwrap().as_u64().unwrap()).collect()
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
    let json = json::Json::from_str(&get_revision(page, id)).unwrap();

    // TODO: try!() isntead of unwrap()? I genuinely don't know.
    let pages = json.as_object().unwrap().get("query").unwrap().as_object().unwrap().get("pages").unwrap().as_object().unwrap();
    let key = pages.keys().next().unwrap();
    pages.get(key).unwrap().as_object().unwrap()
        .get("revisions").unwrap().as_array().unwrap()
        .into_iter().next().unwrap().as_object().unwrap()
        .get("*").unwrap().as_string().unwrap().to_string()
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
        .stdout(Stdio::piped()).stderr(Stdio::piped());
    match process.status() {
        Ok(exit_status) if exit_status.success() =>
            Some(String::from_utf8(process.output().unwrap().stdout).unwrap()),
        Ok(_) => None,
        Err(_) => panic!("Could not get diff3 exit status"),
    }
}

fn main() {
    for id in get_vandalism_revision_ids("Zachary_Taylor") {
        println!("\n\n\n\n\n\n\n\n\n\n");
        println!("Revision {}:", id);
        println!("{}\n", get_revision_content("Zachary_Taylor", id))
    }
}

#[cfg(test)]
mod tests {
    use super::merge;

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
