extern crate hyper;
extern crate rustc_serialize;

use std::io::Read;

use hyper::Client;
use hyper::header::Connection;
use rustc_serialize::json;

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

    let pages = json.as_object().unwrap().get("query").unwrap().as_object().unwrap().get("pages").unwrap().as_object().unwrap();
    let key = pages.keys().next().unwrap();
    pages.get(key).unwrap().as_object().unwrap()
        .get("revisions").unwrap().as_array().unwrap()
        .into_iter().next().unwrap().as_object().unwrap()
        .get("*").unwrap().as_string().unwrap().to_string()
}
 
fn main() {
    for id in get_vandalism_revision_ids("Zachary_Taylor") {
        println!("\n\n\n\n\n\n\n\n\n\n");
        println!("Revision {}:", id);
        println!("{}\n", get_revision_content("Zachary_Taylor", id))
    }
}
