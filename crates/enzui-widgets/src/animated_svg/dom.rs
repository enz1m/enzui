//! The document as an owned element tree, written back out as SVG text with
//! some attributes and properties replaced.

use std::collections::HashMap;
use std::fmt::Write as _;

pub(super) const SVG_NS: &str = "http://www.w3.org/2000/svg";
pub(super) const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// The most elements a document may have.
const MAX_ELEMENTS: usize = 200_000;

/// An element or text node index.
pub(super) type NodeId = usize;

#[derive(Debug, Clone)]
pub(super) struct Attribute {
    /// The name as written, with its prefix (`xlink:href`).
    pub(super) qualified: String,
    /// The local name (`href`).
    pub(super) local: String,
    pub(super) namespace: Option<String>,
    pub(super) value: String,
}

#[derive(Debug, Clone)]
pub(super) enum NodeKind {
    Element {
        /// The tag as written, with its prefix.
        qualified: String,
        /// The local name, for the SVG namespace (and unprefixed tags).
        local: String,
        /// Whether the element is in the SVG namespace (or none).
        svg: bool,
        attributes: Vec<Attribute>,
        /// Namespace declarations this element introduces: `(prefix, uri)`.
        namespaces: Vec<(Option<String>, String)>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub(super) struct Node {
    pub(super) kind: NodeKind,
    pub(super) parent: Option<NodeId>,
    pub(super) children: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub(super) struct Document {
    pub(super) nodes: Vec<Node>,
    pub(super) root: NodeId,
    pub(super) ids: HashMap<String, NodeId>,
}

impl Document {
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: u32::try_from(MAX_ELEMENTS * 2).unwrap_or(u32::MAX),
        };
        let xml =
            roxmltree::Document::parse_with_options(text, options).map_err(|e| e.to_string())?;
        let root = xml.root_element();
        if root.tag_name().name() != "svg" {
            return Err("the root element is not <svg>".into());
        }
        let mut document = Self {
            nodes: Vec::new(),
            root: 0,
            ids: HashMap::new(),
        };
        let mut stack = vec![(root, None::<NodeId>)];
        while let Some((node, parent)) = stack.pop() {
            let id = document.nodes.len();
            if id >= MAX_ELEMENTS {
                return Err("the document has too many elements".into());
            }
            let kind = if node.is_element() {
                let tag = node.tag_name();
                let svg = tag.namespace().is_none_or(|ns| ns == SVG_NS);
                let prefix = tag.namespace().and_then(|ns| node.lookup_prefix(ns));
                let qualified = match prefix {
                    Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", tag.name()),
                    _ => tag.name().to_owned(),
                };
                let attributes = node
                    .attributes()
                    .map(|attribute| {
                        let prefix = attribute.namespace().and_then(|ns| node.lookup_prefix(ns));
                        Attribute {
                            qualified: match prefix {
                                Some(prefix) if !prefix.is_empty() => {
                                    format!("{prefix}:{}", attribute.name())
                                }
                                _ => attribute.name().to_owned(),
                            },
                            local: attribute.name().to_owned(),
                            namespace: attribute.namespace().map(str::to_owned),
                            value: attribute.value().to_owned(),
                        }
                    })
                    .collect();
                let inherited: Vec<(Option<&str>, &str)> = node
                    .parent_element()
                    .map(|p| p.namespaces().map(|ns| (ns.name(), ns.uri())).collect())
                    .unwrap_or_default();
                let namespaces = node
                    .namespaces()
                    .filter(|ns| ns.name() != Some("xml"))
                    .filter(|ns| !inherited.contains(&(ns.name(), ns.uri())))
                    .map(|ns| (ns.name().map(str::to_owned), ns.uri().to_owned()))
                    .collect();
                NodeKind::Element {
                    qualified,
                    local: tag.name().to_owned(),
                    svg,
                    attributes,
                    namespaces,
                }
            } else if node.is_text() {
                NodeKind::Text(node.text().unwrap_or_default().to_owned())
            } else {
                continue;
            };
            document.nodes.push(Node {
                kind,
                parent,
                children: Vec::new(),
            });
            if let Some(parent) = parent {
                document.nodes[parent].children.push(id);
            }
            if node.is_element() {
                if let Some(value) = node.attribute("id") {
                    document.ids.entry(value.to_owned()).or_insert(id);
                }
                // Reversed, so children pop in document order.
                let children: Vec<_> = node.children().collect();
                for child in children.into_iter().rev() {
                    stack.push((child, Some(id)));
                }
            }
        }
        Ok(document)
    }

    /// The element's local name, if `node` is an SVG element.
    pub(super) fn svg_name(&self, node: NodeId) -> Option<&str> {
        match &self.nodes[node].kind {
            NodeKind::Element {
                local, svg: true, ..
            } => Some(local),
            _ => None,
        }
    }

    pub(super) fn is_element(&self, node: NodeId) -> bool {
        matches!(self.nodes[node].kind, NodeKind::Element { .. })
    }

    pub(super) fn attributes(&self, node: NodeId) -> &[Attribute] {
        match &self.nodes[node].kind {
            NodeKind::Element { attributes, .. } => attributes,
            NodeKind::Text(_) => &[],
        }
    }

    /// An attribute in no namespace (or `xlink:href` for `href`).
    pub(super) fn attribute(&self, node: NodeId, local: &str) -> Option<&str> {
        let attributes = self.attributes(node);
        attributes
            .iter()
            .find(|a| a.namespace.is_none() && a.local == local)
            .or_else(|| {
                (local == "href").then(|| {
                    attributes
                        .iter()
                        .find(|a| a.namespace.as_deref() == Some(XLINK_NS) && a.local == "href")
                })?
            })
            .map(|a| a.value.as_str())
    }

    /// The element an `href`-style reference (`#id` or `url(#id)`) names.
    pub(super) fn resolve_reference(&self, reference: &str) -> Option<NodeId> {
        let reference = reference.trim();
        let reference = reference
            .strip_prefix("url(")
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or(reference)
            .trim()
            .trim_matches(['"', '\'']);
        self.ids.get(reference.strip_prefix('#')?).copied()
    }

    pub(super) fn element_children(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes[node]
            .children
            .iter()
            .copied()
            .filter(|child| self.is_element(*child))
    }

    pub(super) fn text_content(&self, node: NodeId) -> String {
        let mut text = String::new();
        let mut stack = vec![node];
        while let Some(node) = stack.pop() {
            match &self.nodes[node].kind {
                NodeKind::Text(value) => text.push_str(value),
                NodeKind::Element { .. } => stack.extend(self.nodes[node].children.iter().rev()),
            }
        }
        text
    }

    /// Every element in document order.
    pub(super) fn elements(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).filter(|node| self.is_element(*node))
    }
}

/// How one element is written out.
#[derive(Debug, Default, Clone)]
pub(super) struct ElementOutput {
    /// Attributes to replace or add, by local name (no namespace), with their
    /// value; `None` removes the attribute.
    pub(super) attributes: Vec<(String, Option<String>)>,
    /// The full inline style to write, replacing the `style` attribute.
    pub(super) style: Option<String>,
    /// Leave the element (and its subtree) out.
    pub(super) omit: bool,
}

impl Document {
    /// The document as SVG text, each element changed as `output` says.
    pub(super) fn serialize<'a>(
        &self,
        output: &dyn Fn(NodeId) -> Option<&'a ElementOutput>,
    ) -> String {
        let mut text = String::with_capacity(self.nodes.len() * 64);
        self.write(self.root, output, &mut text);
        text
    }

    fn write<'a>(
        &self,
        node: NodeId,
        output: &dyn Fn(NodeId) -> Option<&'a ElementOutput>,
        text: &mut String,
    ) {
        let NodeKind::Element {
            qualified,
            attributes,
            namespaces,
            ..
        } = &self.nodes[node].kind
        else {
            if let NodeKind::Text(value) = &self.nodes[node].kind {
                escape(value, false, text);
            }
            return;
        };
        let none = ElementOutput::default();
        let change = output(node).unwrap_or(&none);
        if change.omit {
            return;
        }
        text.push('<');
        text.push_str(qualified);
        for (prefix, uri) in namespaces {
            match prefix {
                Some(prefix) => {
                    let _ = write!(text, " xmlns:{prefix}=\"");
                }
                None => text.push_str(" xmlns=\""),
            }
            escape(uri, true, text);
            text.push('"');
        }
        for attribute in attributes {
            let replaced = attribute.namespace.is_none()
                && (change
                    .attributes
                    .iter()
                    .any(|(name, _)| *name == attribute.local)
                    || (attribute.local == "style" && change.style.is_some()));
            if replaced {
                continue;
            }
            let _ = write!(text, " {}=\"", attribute.qualified);
            escape(&attribute.value, true, text);
            text.push('"');
        }
        for (name, value) in &change.attributes {
            if let Some(value) = value {
                let _ = write!(text, " {name}=\"");
                escape(value, true, text);
                text.push('"');
            }
        }
        if let Some(style) = change.style.as_deref().filter(|s| !s.is_empty()) {
            text.push_str(" style=\"");
            escape(style, true, text);
            text.push('"');
        }
        if self.nodes[node].children.is_empty() {
            text.push_str("/>");
            return;
        }
        text.push('>');
        for child in &self.nodes[node].children {
            self.write(*child, output, text);
        }
        text.push_str("</");
        text.push_str(qualified);
        text.push('>');
    }
}

fn escape(value: &str, attribute: bool, text: &mut String) {
    for character in value.chars() {
        match character {
            '&' => text.push_str("&amp;"),
            '<' => text.push_str("&lt;"),
            '>' => text.push_str("&gt;"),
            '"' if attribute => text.push_str("&quot;"),
            '\n' if attribute => text.push_str("&#10;"),
            _ => text.push(character),
        }
    }
}

/// An element as the CSS selector matcher sees it.
#[derive(Clone, Copy)]
pub(super) struct Selectable<'a> {
    pub(super) document: &'a Document,
    pub(super) node: NodeId,
}

impl simplecss::Element for Selectable<'_> {
    fn parent_element(&self) -> Option<Self> {
        let parent = self.document.nodes[self.node].parent?;
        Some(Self {
            document: self.document,
            node: parent,
        })
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let parent = self.document.nodes[self.node].parent?;
        let siblings = &self.document.nodes[parent].children;
        let index = siblings.iter().position(|s| *s == self.node)?;
        siblings[..index]
            .iter()
            .rev()
            .find(|s| self.document.is_element(**s))
            .map(|s| Self {
                document: self.document,
                node: *s,
            })
    }

    fn has_local_name(&self, name: &str) -> bool {
        match &self.document.nodes[self.node].kind {
            NodeKind::Element { local, .. } => local == name,
            NodeKind::Text(_) => false,
        }
    }

    fn attribute_matches(
        &self,
        local_name: &str,
        operator: simplecss::AttributeOperator<'_>,
    ) -> bool {
        self.document
            .attribute(self.node, local_name)
            .is_some_and(|value| operator.matches(value))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass<'_>) -> bool {
        match class {
            simplecss::PseudoClass::FirstChild => self.prev_sibling_element().is_none(),
            _ => false,
        }
    }
}
