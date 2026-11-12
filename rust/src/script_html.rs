use anyhow::{anyhow, Context, Result};
use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};
use serde_json::{json, Value};
use std::collections::VecDeque;

struct Document {
    html: Html,
    elements: Vec<NodeId>,
    nodes: Vec<NodeId>,
}
#[derive(Default)]
pub struct Documents {
    documents: VecDeque<(i64, Document)>,
}

impl Documents {
    pub fn handle(&mut self, data: &Value) -> Result<Value> {
        let function = data["function"].as_str().context("Missing HTML function")?;
        if function == "parse" {
            let key = data["key"].as_i64().context("Missing document key")?;
            self.documents.retain(|(id, _)| *id != key);
            if self.documents.len() >= 9 {
                self.documents.pop_front();
            }
            self.documents.push_back((
                key,
                Document {
                    html: Html::parse_document(data["data"].as_str().context("Missing HTML")?),
                    elements: vec![],
                    nodes: vec![],
                },
            ));
            return Ok(Value::Null);
        }
        let document = data
            .get("doc")
            .unwrap_or(&data["key"])
            .as_i64()
            .context("Missing document key")?;
        if function == "dispose" {
            self.documents.retain(|(key, _)| *key != document);
            return Ok(Value::Null);
        }
        let doc = &mut self
            .documents
            .iter_mut()
            .find(|(key, _)| *key == document)
            .context("HTML document is closed or expired")?
            .1;
        let selector = || -> Result<Selector> {
            Selector::parse(data["query"].as_str().context("Missing selector")?)
                .map_err(|e| anyhow!("Invalid CSS selector: {e:?}"))
        };
        let index = data["key"].as_u64().unwrap_or(u64::MAX) as usize;
        if function == "querySelector" || function == "querySelectorAll" {
            let ids: Vec<_> = doc.html.select(&selector()?).map(|e| e.id()).collect();
            return Ok(doc.store_elements(ids, function == "querySelector"));
        }
        if function == "getElementById" {
            let id = data["id"].as_str().context("Missing element ID")?;
            let found = doc
                .html
                .tree
                .nodes()
                .filter_map(ElementRef::wrap)
                .find(|e| e.value().id() == Some(id))
                .map(|e| e.id());
            return Ok(doc.store_elements(found.into_iter().collect(), true));
        }
        if function.starts_with("node_") {
            let node = doc
                .html
                .tree
                .get(*doc.nodes.get(index).context("Invalid node handle")?)
                .context("Node removed")?;
            return match function {
                "node_text" => Ok(match node.value() {
                    Node::Text(text) => text.text.to_string().into(),
                    Node::Comment(comment) => comment.comment.to_string().into(),
                    Node::Element(_) => ElementRef::wrap(node)
                        .unwrap()
                        .text()
                        .collect::<String>()
                        .into(),
                    _ => Value::Null,
                }),
                "node_type" => Ok(match node.value() {
                    Node::Text(_) => "text",
                    Node::Comment(_) => "comment",
                    Node::Element(_) => "element",
                    Node::Document => "document",
                    _ => "unknown",
                }
                .into()),
                "node_to_element" => {
                    let id = ElementRef::wrap(node).map(|n| n.id());
                    Ok(doc.store_elements(id.into_iter().collect(), true))
                }
                _ => Err(anyhow!("Unknown HTML node operation")),
            };
        }
        let element = ElementRef::wrap(
            doc.html
                .tree
                .get(*doc.elements.get(index).context("Invalid element handle")?)
                .context("Element removed")?,
        )
        .context("Not an element")?;
        match function {
            "getText" => Ok(element.text().collect::<String>().into()),
            "getInnerHTML" => Ok(element.inner_html().into()),
            "getAttributes" => Ok(Value::Object(
                element
                    .value()
                    .attrs()
                    .map(|(k, v)| (k.to_owned(), v.into()))
                    .collect(),
            )),
            "getClassNames" => Ok(json!(element.value().classes().collect::<Vec<_>>())),
            "getId" => Ok(element.value().id().unwrap_or("").into()),
            "getLocalName" => Ok(element.value().name().into()),
            "getNodes" => {
                let ids: Vec<_> = element.children().map(|n| n.id()).collect();
                let start = doc.nodes.len();
                doc.nodes.extend(ids);
                Ok(json!((start..doc.nodes.len()).collect::<Vec<_>>()))
            }
            "dom_querySelector" | "dom_querySelectorAll" => {
                let ids = element.select(&selector()?).map(|e| e.id()).collect();
                Ok(doc.store_elements(ids, function == "dom_querySelector"))
            }
            "getChildren" => {
                let ids = element
                    .children()
                    .filter_map(ElementRef::wrap)
                    .map(|e| e.id())
                    .collect();
                Ok(doc.store_elements(ids, false))
            }
            "getParent" | "getPreviousSibling" | "getNextSibling" => {
                let found = match function {
                    "getParent" => element.parent().and_then(ElementRef::wrap),
                    "getPreviousSibling" => element.prev_siblings().find_map(ElementRef::wrap),
                    _ => element.next_siblings().find_map(ElementRef::wrap),
                }
                .map(|e| e.id());
                Ok(doc.store_elements(found.into_iter().collect(), true))
            }
            _ => Err(anyhow!("Unknown HTML operation: {function}")),
        }
    }
}
impl Document {
    fn store_elements(&mut self, mut ids: Vec<NodeId>, first: bool) -> Value {
        if first {
            ids.truncate(1);
        }
        let start = self.elements.len();
        self.elements.extend(ids);
        if first {
            if self.elements.len() > start {
                json!(start)
            } else {
                Value::Null
            }
        } else {
            json!((start..self.elements.len()).collect::<Vec<_>>())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_text_nodes_and_disposal() -> Result<()> {
        let mut docs = Documents::default();
        docs.handle(&json!({"function":"parse","key":7,"data":"<div id='a'>Hello<b>world</b><!--comment--></div>"}))?;
        let key = docs.handle(&json!({"function":"querySelector","key":7,"query":"#a"}))?;
        assert_eq!(
            docs.handle(&json!({"function":"getText","doc":7,"key":key}))?,
            "Helloworld"
        );
        assert_eq!(
            docs.handle(&json!({"function":"getNodes","doc":7,"key":key}))?,
            json!([0, 1, 2])
        );
        assert_eq!(
            docs.handle(&json!({"function":"node_type","doc":7,"key":2}))?,
            "comment"
        );
        docs.handle(&json!({"function":"dispose","key":7}))?;
        assert!(docs
            .handle(&json!({"function":"getText","doc":7,"key":key}))
            .is_err());
        Ok(())
    }
}
