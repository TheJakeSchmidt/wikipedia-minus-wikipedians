extern crate redis;

use std::io::Read;
use std::sync::Arc;

use hyper::Client;
use hyper::header::Connection;
use redis::Commands;
use redis::ConnectionInfo;
use rustc_serialize::json::Json;
use url::percent_encoding;

use ::json;
use ::json::JsonPathElement::{Key, Only};

#[derive(Clone)]
pub struct Wiki {
    pub hostname: String,
    pub port: u16,
    client: Arc<Client>,
    redis_connection_info: Option<ConnectionInfo>,
}

#[derive(Clone)]
pub struct Revision {
    pub revid: u64,
    pub parentid: u64,
    pub comment: String,
}

impl Wiki {
    /// Constructs a Wiki object representing the wiki at `hostname` (e.g. "en.wikipedia.org").
    pub fn new(hostname: String, port: u16, client: Client,
               redis_connection_info: Option<ConnectionInfo>)
               -> Wiki {
        Wiki {
            hostname: hostname,
            port: port,
            client: Arc::new(client),
            redis_connection_info: redis_connection_info,
        }
    }

    // TODO: implement a connection pool, or per-thread connections. I tried to do this several ways
    // and failed (redis::Connection isn't Send or Sync, and I couldn't get thread-locals to work).
    // Note: Panics if called when `self.redis_connection_info` is `None`.
    fn get_redis_connection(&self) -> redis::Connection {
        // TODO: delete the format!();
        let _timer = ::Timer::new(format!("Connected to Redis"));
        // The redis-rs docs "heavily encourage" the use of URLs instead of the
        // ConnectionInfo struct, but redis::IntoConnectionInfo is only implemented for
        // &str, so I can't construct a URL and pass it in without using String::as_str(),
        // which is marked unstable.
        let redis_client =
            redis::Client::open((&self.redis_connection_info).clone().unwrap()).unwrap();
        redis_client.get_connection().unwrap()
    }

    fn try_get_cached_value(&self, key: String) -> Option<String> {
        if self.redis_connection_info.is_none() {
            return None;
        }
        // TODO: distinguish errors other than not-found, and log them (but still return None).
        self.get_redis_connection().get(key).ok()
    }

    fn try_cache_value(&self, key: String, value: String) {
        if self.redis_connection_info.is_some() {
            // TODO: log errors here
            let _: redis::RedisResult<String> = self.get_redis_connection().set(key, value);
        }
    }

    /// Calls the MediaWiki API with the given parameters and format=json. Returns the raw JSON.
    fn call_mediawiki_api(&self, parameters: Vec<(&str, &str)>, cacheable: bool)
                          -> Result<String, String> {
        let query =
            parameters.into_iter().map(|p| format!("{}={}", p.0, p.1))
            .collect::<Vec<_>>().join("&") + "&format=json";

        if cacheable {
            match self.try_get_cached_value(query.clone()) {
                Some(result) => return Ok(result),
                _ => (),
            }
        }

        let mut response = try_display!(
            self.client.post(&format!("https://{}/w/api.php", self.hostname))
                .body(&query).header(Connection::close()).send(), "Error calling Wikimedia API");
        let mut body = String::new();
        match response.read_to_string(&mut body) {
            Ok(..) => {
                // TODO: make this asynchronous
                if cacheable {
                    self.try_cache_value(query.clone(), body.clone())
                }
                Ok(body)
            },
            Err(error) =>
                Err(format!("Error converting Wikimedia API response to UTF-8: {}", error)),
        }
    }

    /// Returns the last `limit` revisions for the page `title`.
    pub fn get_revisions(&self, title: &str, limit: u64) -> Result<Vec<Revision>, String> {
        let _timer = ::Timer::new(format!("Got {} revisions of \"{}\"", limit, &title));
        let json_str = try!(self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title),
                 ("rvprop", "comment|ids"), ("rvlimit", &limit.to_string())], false));
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
        let _timer = ::Timer::new(format!("Got latest revision of \"{}\"", &title));
        let mut revisions = try!(self.get_revisions(title, 1));
        revisions.pop().ok_or(format!("No revisions found for page \"{}\"", title))
    }

    /// Returns the contents of the page `title` as of (i.e., immediately after) revision `id`.
    pub fn get_revision_content(&self, title: &str, id: u64) -> Result<String, String> {
        let _timer = ::Timer::new(format!("Got content of revision {} of \"{}\"", &id, &title));
        let json_str = try!(self.call_mediawiki_api(
            vec![("action", "query"), ("prop", "revisions"), ("titles", title), ("rvprop", "content"),
                 ("rvlimit", "1"), ("rvstartid", &id.to_string())], true));
        let json = try_display!(
            Json::from_str(&json_str),
            "Error parsing API response for content of \"{}\" revision {}", title, id);
        Ok(try!(json::get_json_string(
            &json,
            &[Key("query"), Key("pages"), Only, Key("revisions"), Only, Key("*")])).to_string())
    }

    /// Follows all redirects to find the canonical name of the page at `title`.
    pub fn get_canonical_title(&self, title: &str) -> Result<String, String> {
        let _timer = ::Timer::new(format!("Got canonical title of \"{}\"", &title));
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
        let _timer = ::Timer::new(format!("Parsed wikitext for \"{}\"", &title));
        let encoded_wikitext =
            percent_encoding::percent_encode(
                wikitext.as_bytes(), percent_encoding::FORM_URLENCODED_ENCODE_SET);
        let response = try!(self.call_mediawiki_api(
            vec![("action", "parse"), ("prop", "text"), ("disablepp", ""),
                 ("contentmodel", "wikitext"), ("title", title), ("text", &encoded_wikitext)], true));
        let json = try_display!(
            Json::from_str(&response),
            "Error parsing API response for parsing merged wikitext of \"{}\"", title);
        Ok(try!(json::get_json_string(&json, &[Key("parse"), Key("text"), Key("*")])).to_string())
    }

    /// Gets the current, fully-rendered (**HTML**) contents of the page `title`.
    pub fn get_current_page_content(&self, title: &str) -> Result<String, String> {
        let _timer = ::Timer::new(format!("Got current HTML contents of \"{}\"", &title));
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
