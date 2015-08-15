//! Contains all HTML-related code. See the `Page` documentation for details.

// TODO: Why do I need these two lines "extern crate html5ever" and "extern crate
// html5ever_dom_sink" both here and in main.rs?
extern crate html5ever;
extern crate html5ever_dom_sink;
extern crate rand;
extern crate tendril;

use std::str::FromStr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use html5ever::Attribute;
use html5ever::tree_builder::interface::TreeSink;
use html5ever_dom_sink::common::NodeEnum;
use html5ever_dom_sink::rcdom::Handle;
use html5ever_dom_sink::rcdom::RcDom;
use regex::Captures;

use wiki::Wiki;

// TODO: massive cleanup, all over this file.
// TODO: I'm using "body" and even "html_body" to mean "article body" in this file and main.rs,
// which isn't right. Come up with a better name for that and use it everywhere.

/// Represents, and owns all behavior related to, the contents of the HTML page shown to the
/// user. This includes fetching the rendered article from Wikipedia, replacing its contents with
/// the rendered wikitext, and processing/removing merge markers.
///
/// The API to this struct comprises two methods: Page::new() constructs a new Page. It should be
/// called early, so the Page can start fetching article HTML from Wikipedia.
/// Page:replace_body_and_remove_merge_markers() processes the merge markers in the rendered
/// wikitext and puts the header and footer around it.
pub struct Page {
    placeholder: String,
    page_skeleton_receiver: Receiver<Result<String, String>>,
}

impl Page {
    /// Creates a new Page representing the article at `title`. This kicks off a background thread
    /// that fetches the current article HTML from Wikipedia. Because of that, it should be called
    /// as early as possible (as soon as the title being served is known), so that the page fetch
    /// stays off the critical path for page load.
    pub fn new(title: &str, wiki: Wiki) -> Page {
        let placeholder = format!("WMW_PLACEHOLDER_{}", rand::random::<u64>());
        let page_skeleton_receiver =
            Page::spawn_page_skeleton_fetch_thread(title, placeholder.clone(), wiki);
        Page {
            placeholder: placeholder,
            page_skeleton_receiver: page_skeleton_receiver,
        }
    }

    /// This finishes the HTML processing - it replaces the merge markers in `html_body` with HTML
    /// tags, and inserts the resulting HTML into the page skeleton.
    pub fn replace_body_and_remove_merge_markers(&self, html_body: String)
                                                 -> Result<String, String> {
        match self.page_skeleton_receiver.recv() {
            Ok(Ok(page_skeleton)) => {
                // TODO: This should remove the merge markers first, and replace the page
                // contents second. There are never merge markers in the page skeleton, so it's
                // useless to look for merge markers there.
                let html_with_merge_markers =
                    remove_merge_markers_from_html(
                        page_skeleton.replace(&self.placeholder, &html_body));
                // TODO: Move this elsewhere, use constants, etc.
                let start_regex = regex!("\u{E000}([0-9]+)\u{E000}");
                let end_regex = regex!("\u{E001}[0-9]+\u{E001}");
                let finished_html =
                    start_regex.replace_all(
                        &end_regex.replace_all(&html_with_merge_markers, "</span>"),
                        |captures: &Captures|
                        format!("<span style=\"color: red\" class=\"vandalism-{}\">",
                                captures.at(1).unwrap()));
                Ok(finished_html)
            },
            Ok(Err(msg))=> Err(msg),
            Err(err) => Err(format!("error: {}", err)),
        }
    }

    fn spawn_page_skeleton_fetch_thread(title: &str, placeholder: String, wiki: Wiki)
                                  -> Receiver<Result<String, String>> {
        let (page_skeleton_sender, page_skeleton_receiver) = channel::<Result<String, String>>();
        let title = title.to_owned().clone();
        thread::spawn(move|| {
            page_skeleton_sender.send(
                match wiki.get_current_page_content(&title) {
                    Ok(content) =>
                        replace_node_with_placeholder(&content, "mw-content-text", &placeholder),
                    Err(msg) => Err(msg),
                }).unwrap();
        });
        page_skeleton_receiver
    }
}

fn replace_node_with_placeholder(original_html: &str, div_id: &str, placeholder: &str)
    -> Result<String, String> {
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
    try_display!(
        html5ever::serialize::serialize(&mut serialized, &dom.document, Default::default()),
        "Failed to serialize modified HTML");
    Ok(try_display!(String::from_utf8(serialized),
                    "Error converting serialized HTML to UTF-8 string"))
}

fn find_node_by_id(handle: &Handle, id: &str) -> Result<Handle, String> {
    fn has_matching_id(attributes: &Vec<Attribute>, id: &str) -> bool {
        return attributes.into_iter().any(
            |attribute| attribute.name.local.as_slice() == "id" &&
                format!("{}", attribute.value) == id);
    }

    let node = handle.borrow();
    match node.node {
        NodeEnum::Element(_, ref attributes) if has_matching_id(attributes, id) => Ok(handle.clone()),
        _ => (&node.children).into_iter()
            .map(|child| find_node_by_id(child, id))
            .filter(|result| result.is_ok())
            .map(|result| result.unwrap())
            .next().ok_or(format!("No node with ID {} found", id)),
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_merge_markers_from_html, replace_node_with_placeholder};

    #[test]
    fn test_remove_merge_markers_from_html() {
        let html = format!("<html><body>{}123{}<img src=\"asdf{}123{}.jpg\"></body></html>",
                           ::START_MARKER, ::START_MARKER, ::END_MARKER, ::END_MARKER);
        let expected = "<html><body><img src=\"asdf.jpg\"></body></html>";
        assert_eq!(expected, remove_merge_markers_from_html(html));
    }

    #[test]
    fn test_remove_merge_markers_from_html_keep() {
        let html = format!("<html><body>{}456{}<img src=\"asdf.jpg\">{}456{}</body></html>",
                           ::START_MARKER, ::START_MARKER, ::END_MARKER, ::END_MARKER);
        assert_eq!(html.clone(), remove_merge_markers_from_html(html));
    }

    #[test]
    fn test_remove_merge_markers_from_html_keep_one_remove_one() {
        let html = format!(
            "<html><body>{}234{}<b>text{}234{}</b>{}567{}<img src=\"asdf{}567{}.jpg\"></body></html>",
            ::START_MARKER, ::START_MARKER, ::END_MARKER, ::END_MARKER, ::START_MARKER, ::START_MARKER,
            ::END_MARKER, ::END_MARKER);
        let expected = format!(
            "<html><body>{}234{}<b>text{}234{}</b><img src=\"asdf.jpg\"></body></html>",
            ::START_MARKER, ::START_MARKER, ::END_MARKER, ::END_MARKER);
        assert_eq!(expected, remove_merge_markers_from_html(html));
    }

    #[test]
    fn test_replace_html_content() {
        let original_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\"><p>original text</p></div><div>Other text</div></div></div></body></html>";
        let expected_html = "<html><head></head><body><div id=\"content\"><div id=\"bodyContent\"><div id=\"mw-content-text\">replaced text</div><div>Other text</div></div></div></body></html>";
        let processed_html = replace_node_with_placeholder(original_html, "mw-content-text", "replaced text").unwrap();
        assert_eq!(expected_html, processed_html);
    }
}

fn remove_merge_markers_from_html(html: String) -> String {
    // TODO: clean up this whole function. regex[1..4] are not good names.
    // TODO: use START_MARKER and END_MARKER constants here.
    // Finds markers where the end, but not the start, is inside a tag.
    let regex1 = regex!(
        r"\x{E000}[0-9]+\x{E000}([^\x{E001}]*?)<([^>]*?)\x{E001}[0-9]+\x{E001}([^>]*?)>");
    // Finds markers where the start, but not the end, is inside a tag.
    let regex2 = regex!(
        r"<([^>]*?)\x{E000}[0-9]+\x{E000}([^>]*?)>([^\x{E000}]*?)\x{E000}[0-9]+\x{E000}");
    // Finds markers where both the start and end are inside tags.
    let regex3 = regex!(
        r"<([^>]*?)\x{E000}[0-9]+\x{E000}([^>]*?)>([^\x{E000}\x{E001}]*?)<([^>]*?)\x{E001}[0-9]+\x{E001}([^>]*?)>");

    let html1 = regex1.replace_all(
        &html, |captures: &Captures|
        format!("{}<{}{}>", captures.at(1).unwrap(), captures.at(2).unwrap(),
                captures.at(3).unwrap()));
    let html2 = regex2.replace_all(
        &html1, |captures: &Captures|
        format!("<{}{}>{}>", captures.at(1).unwrap(), captures.at(2).unwrap(),
                captures.at(3).unwrap()));
    regex3.replace_all(
        &html2, |captures: &Captures|
        format!("<{}{}>{}<{}{}>", captures.at(1).unwrap(), captures.at(2).unwrap(),
                captures.at(3).unwrap(), captures.at(4).unwrap(), captures.at(5).unwrap()))
}
