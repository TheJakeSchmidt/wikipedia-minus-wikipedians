use std::io::Read;

use hyper::Client;
use hyper::header::Connection;
use rustc_serialize::json::Json;
use url::percent_encoding;

use ::json;
use ::json::JsonPathElement::{Key, Only};

pub struct Wiki {
    pub hostname: String,
    client: Client,
}

#[derive(Clone)]
pub struct Revision {
    pub revid: u64,
    pub parentid: u64,
    pub comment: String,
}

impl Wiki {
    /// Constructs a Wiki object representing the wiki at `hostname` (e.g. "en.wikipedia.org").
    pub fn new(hostname: String, client: Client) -> Wiki {
        Wiki { hostname: hostname, client: client }
    }

    /// Calls the MediaWiki API with the given parameters and format=json. Returns the raw JSON.
    fn call_mediawiki_api(&self, parameters: Vec<(&str, &str)>) -> Result<String, String> {
        let post_body =
            parameters.into_iter().map(|p| format!("{}={}", p.0, p.1))
            .collect::<Vec<_>>().join("&") + "&format=json";

        let mut response = try_display!(
            self.client.post(&format!("https://{}/w/api.php", self.hostname))
                .body(&post_body).header(Connection::close()).send(), "Error calling Wikimedia API");
        let mut body = String::new();
        match response.read_to_string(&mut body) {
            Ok(..) => Ok(body),
            Err(error) =>
                Err(format!("Error converting Wikimedia API response to UTF-8: {}", error)),
        }
    }

    /// Returns the last `limit` revisions for the page `title`.
    pub fn get_revisions(&self, title: &str, limit: u64) -> Result<Vec<Revision>, String> {
        let json_str = try!(self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title),
                 ("rvprop", "comment|ids"), ("rvlimit", &limit.to_string())]));
        let json = try_display!(
            Json::from_str(&json_str),
            "Error parsing API response for {} revisions of \"{}\"", limit, title);
        let revisions_json = try!(
            json::get_json_array(&json, &[Key("query"), Key("pages"), Only, Key("revisions")]));

        let mut revisions = Vec::with_capacity(revisions_json.len());
        for revision_json in revisions_json {
            revisions.push(
                Revision {
                    revid: try!(json::get_json_number(revision_json, &[Key("revid")])),
                    parentid: try!(json::get_json_number(revision_json, &[Key("parentid")])),
                    comment: try!(json::get_json_string(revision_json, &[Key("comment")])).to_string()
                });
        }
        Ok(revisions)
    }

    /// Returns the latest revision ID for the page `title`.
    pub fn get_latest_revision(&self, title: &str) -> Result<Revision, String> {
        let mut revisions = try!(self.get_revisions(title, 1));
        revisions.pop().ok_or(format!("No revisions found for page \"{}\"", title))
    }

    /// Returns the contents of the page `title` as of (i.e., immediately after) revision `id`.
    pub fn get_revision_content(&self, title: &str, id: u64) -> Result<String, String> {
        let json_str = try!(self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title), ("rvprop", "content"),
                 ("rvlimit", "1"), ("rvstartid", &id.to_string())]));
        let json = try_display!(
            Json::from_str(&json_str),
            "Error parsing API response for content of \"{}\" revision {}", title, id);
        Ok(try!(json::get_json_string(
            &json,
            &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("*")])).to_string())
    }

    /// Follows all redirects to find the canonical name of the page at `title`.
    pub fn get_canonical_title(&self, title: &str) -> Result<String, String> {
        let latest_revision_id = try!(self.get_latest_revision(title)).revid;
        let page_contents = try!(self.get_revision_content(title, latest_revision_id));

        let regex = regex!(r"#REDIRECT \[\[([^]]+)\]\].*");
        match regex.captures(&page_contents) {
            Some(captures) => self.get_canonical_title(captures.at(1).unwrap()),
            None => Ok(title.to_string()),
        }
    }

    /// Parses the wikitext in `wikitext` as though it were the contents of the page `title`,
    /// returning the rendered HTML.
    pub fn parse_wikitext(&self, title: &str, wikitext: &str) -> Result<String, String> {
        let encoded_wikitext =
            percent_encoding::percent_encode(
                wikitext.as_bytes(), percent_encoding::FORM_URLENCODED_ENCODE_SET);
        let response = try!(self.call_mediawiki_api(
            vec![("action", "parse"), ("prop", "text"), ("disablepp", ""), ("contentmodel", "wikitext"),
                 ("title", title), ("text", &encoded_wikitext)]));
        // TODO: check return value
        let json = try_display!(
            Json::from_str(&response),
            "Error parsing API response for parsing merged wikitext of \"{}\"", title);
        Ok(try!(json::get_json_string(&json, &[Key("parse"), Key("text"), Key("*")])).to_string())
    }

    /// Gets the current, fully-rendered (**HTML**) contents of the page `title`.
    pub fn get_current_page_content(&self, title: &str) -> Result<String, String> {
        let url = format!("https://{}/wiki/{}", self.hostname, title);
        let mut response =
            try_display!(
                self.client.get(&url).header(Connection::close()).send(),
                "Error fetching URL {}", url);
        let mut body = String::new();
        match response.read_to_string(&mut body) {
            Ok(..) => Ok(body),
            Err(error) => Err(format!("{}", error))
        }
    }
}
