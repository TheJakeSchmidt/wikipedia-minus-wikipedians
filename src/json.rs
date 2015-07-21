extern crate rustc_serialize;

use rustc_serialize::json::Json;
use rustc_serialize::json::Json::{Array, Object};

// TODO: doc comments

pub enum JsonPathElement {
    Key(&'static str), // Use the value associated with a specific key.
    Only the only value in an array or object.
}

use json::JsonPathElement::Key;
use json::JsonPathElement::Only;

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

pub fn get_json_array<'a>(json: &'a Json, path: &[JsonPathElement]) -> Result<&'a Vec<Json>, String> {
    match get_json_value(json, path, 0) {
        Ok(&Json::Array(ref value)) => Ok(value),
        Ok(..) => Err(format!("Asked for array {}, but value is not an array",
                              pretty_print(&path[..]))),
        Err(message) => Err(message),
    }
}

pub fn get_json_number(json: &Json, path: &[JsonPathElement]) -> Result<u64, String> {
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

pub fn get_json_string<'a>(json: &'a Json, path: &[JsonPathElement]) -> Result<&'a str, String> {
    match get_json_value(json, path, 0) {
        Ok(&Json::String(ref value)) => Ok(value),
        Ok(..) => Err(format!("Asked for string {}, but value is not a string",
                              pretty_print(&path[..]))),
        Err(message) => Err(message),
    }
}

#[cfg(test)]
mod tests {
    use super::{get_json_array, get_json_string, get_json_number};
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
}
