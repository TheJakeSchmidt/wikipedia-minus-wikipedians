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

// TODO: doc comments everywhere!
pub struct Page {
    html_body_sender: Sender<String>,
    replaced_body_receiver: Receiver<Result<String, String>>,
}

impl Page {
    pub fn new(title: &str, wiki: Wiki) -> Page {
        let placeholder = format!("WMW_PLACEHOLDER_{}", rand::random::<u64>());
        let current_content_receiver =
            Page::spawn_content_fetch_thread(title, placeholder.clone(), wiki);
        let (html_body_sender, replaced_body_receiver) =
            Page::spawn_replace_body_thread(current_content_receiver, placeholder);
        Page {
            html_body_sender: html_body_sender,
            replaced_body_receiver: replaced_body_receiver,
        }
    }

    pub fn replace_body_and_remove_merge_markers(&self, html_body: String)
                                                 -> Result<String, String> {
        self.html_body_sender.send(html_body);
        try_display!(self.replaced_body_receiver.recv(), "Failed to get data from channel")
    }

    fn spawn_content_fetch_thread(title: &str, placeholder: String, wiki: Wiki)
                                  -> Receiver<Result<String, String>> {
        let (sender, receiver) = channel::<Result<String, String>>();
        let title = title.to_owned().clone();
        thread::spawn(move|| {
            sender.send(
                match wiki.get_current_page_content(&title) {
                    Ok(content) =>
                        replace_node_with_placeholder(&content, "mw-content-text", &placeholder),
                Err(msg) => Err(msg),
            }).unwrap();
        });
        receiver
    }

    // TODO: doc comment
    fn spawn_replace_body_thread(
        page_skeleton_receiver: Receiver<Result<String, String>>, placeholder: String)
        -> (Sender<String>, Receiver<Result<String, String>>) {
        let (html_body_sender, html_body_receiver) = channel::<String>();
        let (replaced_body_sender, replaced_body_receiver) = channel::<Result<String, String>>();
        thread::spawn(move|| {
            match (page_skeleton_receiver.recv(), html_body_receiver.recv()) {
                (Ok(Ok(page_skeleton)), Ok(body)) => {
                    let html_with_merge_markers =
                        remove_merge_markers_from_html(page_skeleton.replace(&placeholder, &body));
                    // TODO: Move this elsewhere, use constants, etc.
                    let start_regex = regex!("\u{E000}([0-9]+)\u{E000}");
                    let end_regex = regex!("\u{E001}[0-9]+\u{E001}");
                    let finished_html =
                        start_regex.replace_all(
                            &end_regex.replace_all(&html_with_merge_markers, "</span>"),
                            |captures: &Captures|
                            format!("<span style=\"color: red\" class=\"vandalism-{}\">",
                                    captures.at(1).unwrap()));
                    replaced_body_sender.send(Ok(finished_html))
                },
                (Ok(Err(msg)), _) => replaced_body_sender.send(Err(msg)),
                (Err(err), _) => replaced_body_sender.send(Err(format!("error: {}", err))),
                (_, Err(err)) => replaced_body_sender.send(Err(format!("error: {}", err))),
            }.unwrap()
        });
        (html_body_sender, replaced_body_receiver)
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
