//! Matching parsed selectors against elements of a [`Document`].
//!
//! Complex selectors are matched right to left: the subject compound
//! against the element, then each combinator walks to the candidates for the
//! next compound. The walk uses the classic early-exit results (as in
//! WebKit and servo) so that selectors like `a b c d` don't backtrack
//! through every combination of ancestors.
//!
//! dasher has no layout, focus, hover or history, so pseudo-classes about
//! those either never match or are decided statically from the markup; see
//! the crate docs for the full list.

use dasher_dom::{Document, Name, NodeId, NodeKind, QuirksMode};

use crate::selector::{
    AttrCase, AttrOp, AttrSelector, Combinator, Complex, LocalName, NameRef, Nth, Part, Pseudo,
    Reach, Relative, SelectorList, Simple,
};

/// The outcome of matching part of a complex selector, telling the
/// combinator loops further right whether trying more candidates can help.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Match {
    Matched,
    /// Try the next candidate of the nearest `~` or descendant combinator.
    RetryLaterSibling,
    /// Siblings won't help: try the next candidate of the nearest
    /// descendant combinator.
    RetryDescendant,
    /// No candidate further on can match.
    Never,
}

/// What matching needs besides the selector and the element.
pub(crate) struct Context<'a> {
    pub(crate) doc: &'a Document,
    /// The element `:scope` matches; `None` makes it match `:root`.
    scope: Option<NodeId>,
    quirks: bool,
}

impl<'a> Context<'a> {
    /// `scope` is the scoping root: the element or node a query was made
    /// on. When it is the document itself, or there is none, `:scope`
    /// matches the root element.
    pub(crate) fn new(doc: &'a Document, scope: Option<NodeId>) -> Self {
        Context {
            doc,
            scope: scope.filter(|&s| doc.kind(s) != NodeKind::Document),
            quirks: doc.quirks_mode() == QuirksMode::Quirks,
        }
    }

    /// Whether the element `el` matches any selector in `list`.
    pub(crate) fn matches_list(&self, list: &SelectorList, el: NodeId) -> bool {
        list.selectors
            .iter()
            .any(|complex| self.matches_complex(complex, el, None))
    }

    fn matches_complex(&self, complex: &Complex, el: NodeId, anchor: Option<NodeId>) -> bool {
        self.match_from(&complex.parts, 0, el, anchor) == Match::Matched
    }

    /// Matches `parts[k..]` with `parts[k]` against `el`.
    fn match_from(&self, parts: &[Part], k: usize, el: NodeId, anchor: Option<NodeId>) -> Match {
        let part = &parts[k];
        if !part
            .compound
            .iter()
            .all(|simple| self.matches_simple(simple, el, anchor))
        {
            return Match::RetryLaterSibling;
        }
        if k + 1 == parts.len() {
            return Match::Matched;
        }

        let combinator = part.combinator;
        let mut current = el;
        loop {
            let candidate = match combinator {
                Combinator::Child | Combinator::Descendant => self.doc.parent_element(current),
                Combinator::NextSibling | Combinator::SubsequentSibling => {
                    self.prev_element_sibling(current)
                }
            };
            let Some(candidate) = candidate else {
                // Out of candidates. If this was a sibling combinator, a
                // different element further right may have other siblings;
                // ancestors are shared by everything further right.
                return if combinator.is_sibling() {
                    Match::RetryDescendant
                } else {
                    Match::Never
                };
            };
            let result = self.match_from(parts, k + 1, candidate, anchor);
            match (result, combinator) {
                (Match::Matched | Match::Never, _) | (_, Combinator::NextSibling) => {
                    return result;
                }
                (_, Combinator::Child) => return Match::RetryDescendant,
                (Match::RetryDescendant, Combinator::SubsequentSibling) => return result,
                _ => current = candidate,
            }
        }
    }

    fn matches_simple(&self, simple: &Simple, el: NodeId, anchor: Option<NodeId>) -> bool {
        let doc = self.doc;
        match simple {
            Simple::Type {
                no_namespace,
                local,
            } => {
                if *no_namespace && doc.namespace(el) != Some(Name::EMPTY) {
                    return false;
                }
                match local {
                    None => true,
                    Some(local) => doc
                        .local_name(el)
                        .is_some_and(|name| self.local_name_matches(local, el, name)),
                }
            }
            Simple::Id(id) => doc.element_id(el).is_some_and(|value| {
                if self.quirks {
                    value.eq_ignore_ascii_case(id)
                } else {
                    value == &**id
                }
            }),
            Simple::Class(class) => doc.has_class(el, class),
            Simple::Attr(attr) => self.matches_attr(attr, el),
            Simple::Pseudo(pseudo) => self.matches_pseudo(pseudo, el),
            Simple::Anchor => Some(el) == anchor,
            Simple::Never => false,
        }
    }

    fn name_matches(&self, wanted: &NameRef, name: Name) -> bool {
        match wanted {
            NameRef::Static(wanted) => *wanted == name,
            NameRef::Other(wanted) => !name.is_static() && self.doc.name_str(name) == &**wanted,
        }
    }

    /// Compares a type or attribute selector's name with a name on `el`:
    /// ASCII case-insensitively for HTML elements, exactly for others.
    fn local_name_matches(&self, local: &LocalName, el: NodeId, name: Name) -> bool {
        let wanted = if self.is_html(el) {
            &local.lower
        } else {
            &local.exact
        };
        self.name_matches(wanted, name)
    }

    fn is_html(&self, el: NodeId) -> bool {
        self.doc.namespace(el) == Some(Name::NS_HTML)
    }

    fn matches_attr(&self, sel: &AttrSelector, el: NodeId) -> bool {
        let doc = self.doc;
        let html = self.is_html(el);
        doc.attrs(el).iter().any(|attr| {
            let no_namespace = doc.attr_namespace(attr) == Name::EMPTY;
            if !(sel.any_namespace || no_namespace)
                || !self.local_name_matches(&sel.local, el, attr.local())
            {
                return false;
            }
            let Some((op, wanted)) = &sel.value else {
                return true;
            };
            let ignore_case = match sel.case {
                AttrCase::Sensitive => false,
                AttrCase::Insensitive => true,
                AttrCase::HtmlLegacy => html && no_namespace,
            };
            attr_value_matches(*op, attr.value(), wanted, ignore_case)
        })
    }

    fn prev_element_sibling(&self, el: NodeId) -> Option<NodeId> {
        std::iter::successors(self.doc.prev_sibling(el), |&n| self.doc.prev_sibling(n))
            .find(|&n| self.doc.is_element(n))
    }

    fn next_element_sibling(&self, el: NodeId) -> Option<NodeId> {
        std::iter::successors(self.doc.next_sibling(el), |&n| self.doc.next_sibling(n))
            .find(|&n| self.doc.is_element(n))
    }

    fn same_type(&self, a: NodeId, b: NodeId) -> bool {
        let doc = self.doc;
        doc.local_name(a) == doc.local_name(b) && doc.namespace(a) == doc.namespace(b)
    }

    fn is_root(&self, el: NodeId) -> bool {
        self.doc
            .parent(el)
            .is_some_and(|p| self.doc.kind(p) == NodeKind::Document)
    }

    fn matches_pseudo(&self, pseudo: &Pseudo, el: NodeId) -> bool {
        let doc = self.doc;
        match pseudo {
            Pseudo::Root => self.is_root(el),
            Pseudo::Empty => doc.children(el).all(|c| match doc.kind(c) {
                NodeKind::Element => false,
                NodeKind::Text => doc.text(c).is_some_and(str::is_empty),
                _ => true,
            }),
            Pseudo::Scope => match self.scope {
                Some(scope) => scope == el,
                None => self.is_root(el),
            },
            Pseudo::OnlyChild => {
                self.prev_element_sibling(el).is_none() && self.next_element_sibling(el).is_none()
            }
            Pseudo::OnlyOfType => {
                let mut siblings = std::iter::successors(self.prev_element_sibling(el), |&n| {
                    self.prev_element_sibling(n)
                })
                .chain(std::iter::successors(self.next_element_sibling(el), |&n| {
                    self.next_element_sibling(n)
                }));
                !siblings.any(|s| self.same_type(s, el))
            }
            Pseudo::Nth(nth) => self.matches_nth(nth, el),
            Pseudo::Not(list) => !self.matches_list(list, el),
            Pseudo::Is(list) => self.matches_list(list, el),
            Pseudo::Has(relatives) => relatives.iter().any(|r| self.matches_relative(r, el)),
            Pseudo::AnyLink => {
                (doc.is_html(el, Name::A) || doc.is_html(el, Name::AREA))
                    && doc.attribute_named(el, Name::HREF).is_some()
                    || doc.is_element_named(el, Name::NS_SVG, Name::A)
                        && (doc.attribute_named(el, Name::HREF).is_some()
                            || doc.attribute_ns(el, Name::NS_XLINK, Name::HREF).is_some())
            }
            Pseudo::Enabled => self.can_be_disabled(el) && !self.is_disabled(el),
            Pseudo::Disabled => self.can_be_disabled(el) && self.is_disabled(el),
            Pseudo::Checked => self.is_checked(el),
            Pseudo::Indeterminate => self.is_indeterminate(el),
            Pseudo::Default => self.is_default(el),
            Pseudo::Required => self.required_applies(el) && self.has(el, Name::REQUIRED),
            Pseudo::Optional => self.required_applies(el) && !self.has(el, Name::REQUIRED),
            Pseudo::ReadWrite => self.is_read_write(el),
            Pseudo::ReadOnly => !self.is_read_write(el),
            Pseudo::PlaceholderShown => self.is_placeholder_shown(el),
            Pseudo::Open => {
                (doc.is_html(el, Name::DETAILS) || doc.is_html(el, Name::DIALOG))
                    && self.has(el, Name::OPEN)
            }
            Pseudo::Defined => true,
            Pseudo::Visible => self.is_visible(el),
            Pseudo::Lang(ranges) => self.matches_lang(ranges, el),
            Pseudo::Dir(dir) => dir.is_some_and(|rtl| self.is_rtl(el) == rtl),
        }
    }

    fn matches_nth(&self, nth: &Nth, el: NodeId) -> bool {
        if let Some(of) = &nth.of {
            if !self.matches_list(of, el) {
                return false;
            }
        }
        let counts = |sibling: NodeId| {
            if nth.of_type {
                self.same_type(sibling, el)
            } else if let Some(of) = &nth.of {
                self.matches_list(of, sibling)
            } else {
                true
            }
        };
        let step = |n: NodeId| {
            if nth.from_end {
                self.next_element_sibling(n)
            } else {
                self.prev_element_sibling(n)
            }
        };
        let mut index: i64 = 1;
        let mut current = el;
        while let Some(sibling) = step(current) {
            if counts(sibling) {
                index += 1;
                // With A <= 0, indices past B can never match.
                if nth.a <= 0 && index > i64::from(nth.b) {
                    return false;
                }
            }
            current = sibling;
        }
        nth.matches_index(index)
    }

    /// `:has()`: whether some element around `anchor` matches the relative
    /// selector.
    fn matches_relative(&self, rel: &Relative, anchor: NodeId) -> bool {
        let doc = self.doc;
        let try_match = |el: NodeId| self.matches_complex(&rel.complex, el, Some(anchor));
        let mut later_siblings = std::iter::successors(self.next_element_sibling(anchor), |&n| {
            self.next_element_sibling(n)
        });
        match rel.reach {
            Reach::Children => doc.element_children(anchor).any(try_match),
            Reach::Descendants => doc
                .descendants(anchor)
                .filter(|&n| doc.is_element(n))
                .any(try_match),
            Reach::NextSibling => self.next_element_sibling(anchor).is_some_and(try_match),
            Reach::Siblings => later_siblings.any(try_match),
            Reach::SiblingsAndDescendants => later_siblings.any(|s| {
                try_match(s)
                    || doc
                        .descendants(s)
                        .filter(|&n| doc.is_element(n))
                        .any(try_match)
            }),
        }
    }

    // -----------------------------------------------------------------
    // Form state, decided from attributes: dasher has no user input, so an
    // element's state is the one its markup gives it.

    fn has(&self, el: NodeId, attr: Name) -> bool {
        self.doc.attribute_named(el, attr).is_some()
    }

    /// An `<input>`'s `type`, lower-cased, defaulting to `text`.
    fn input_type(&self, el: NodeId) -> Option<String> {
        if !self.doc.is_html(el, Name::INPUT) {
            return None;
        }
        let ty = self.doc.attribute_named(el, Name::TYPE).unwrap_or("text");
        Some(ty.to_ascii_lowercase())
    }

    fn can_be_disabled(&self, el: NodeId) -> bool {
        [
            Name::BUTTON,
            Name::INPUT,
            Name::SELECT,
            Name::TEXTAREA,
            Name::FIELDSET,
            Name::OPTGROUP,
            Name::OPTION,
        ]
        .iter()
        .any(|&name| self.doc.is_html(el, name))
    }

    /// HTML's "actually disabled" for elements that can be disabled.
    fn is_disabled(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if self.has(el, Name::DISABLED) {
            return true;
        }
        if doc.is_html(el, Name::OPTGROUP) {
            return false;
        }
        if doc.is_html(el, Name::OPTION) {
            return doc
                .parent(el)
                .is_some_and(|p| doc.is_html(p, Name::OPTGROUP) && self.has(p, Name::DISABLED));
        }
        // Inside a disabled fieldset, except within its first legend.
        let mut child = el;
        for ancestor in doc.ancestors(el) {
            if doc.is_html(ancestor, Name::FIELDSET) && self.has(ancestor, Name::DISABLED) {
                let first_legend = doc
                    .element_children(ancestor)
                    .find(|&c| doc.is_html(c, Name::LEGEND));
                if first_legend != Some(child) {
                    return true;
                }
            }
            child = ancestor;
        }
        false
    }

    fn is_checked(&self, el: NodeId) -> bool {
        match self.input_type(el).as_deref() {
            Some("checkbox" | "radio") => self.has(el, Name::CHECKED),
            Some(_) => false,
            None => self.doc.is_html(el, Name::OPTION) && self.is_selected_option(el),
        }
    }

    /// The `<select>` an `<option>` belongs to, directly or through an
    /// `<optgroup>`.
    fn option_select(&self, option: NodeId) -> Option<NodeId> {
        let doc = self.doc;
        let parent = doc.parent_element(option)?;
        if doc.is_html(parent, Name::SELECT) {
            return Some(parent);
        }
        let grandparent = doc.parent_element(parent)?;
        (doc.is_html(parent, Name::OPTGROUP) && doc.is_html(grandparent, Name::SELECT))
            .then_some(grandparent)
    }

    /// An option's selectedness as the markup leaves it. A drop-down
    /// `<select>` always has one selected option: the last one marked
    /// `selected`, or else the first enabled one.
    fn is_selected_option(&self, option: NodeId) -> bool {
        let doc = self.doc;
        let marked = self.has(option, Name::SELECTED);
        let Some(select) = self.option_select(option) else {
            return marked;
        };
        let size = doc
            .attribute_named(select, Name::SIZE)
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1);
        if self.has(select, Name::MULTIPLE) {
            return marked;
        }
        let options: Vec<NodeId> = doc
            .descendants(select)
            .filter(|&n| doc.is_html(n, Name::OPTION) && self.option_select(n) == Some(select))
            .collect();
        let selected = options
            .iter()
            .rev()
            .find(|&&o| self.has(o, Name::SELECTED))
            .or_else(|| {
                if size > 1 {
                    None
                } else {
                    options.iter().find(|&&o| !self.is_disabled(o))
                }
            });
        selected == Some(&option)
    }

    fn is_indeterminate(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if doc.is_html(el, Name::PROGRESS) {
            return !self.has(el, Name::VALUE);
        }
        if self.input_type(el).as_deref() != Some("radio") {
            return false;
        }
        // A radio button whose group has nothing checked.
        let name = doc.attribute_named(el, Name::NAME).unwrap_or("");
        if name.is_empty() {
            return !self.has(el, Name::CHECKED);
        }
        let owner = self.form_owner(el).unwrap_or_else(|| doc.tree_root(el));
        !doc.descendants(owner).any(|n| {
            self.input_type(n).as_deref() == Some("radio")
                && doc.attribute_named(n, Name::NAME) == Some(name)
                && self.form_owner(n) == self.form_owner(el)
                && self.has(n, Name::CHECKED)
        })
    }

    /// The nearest ancestor `<form>`. The `form` attribute is not
    /// supported.
    fn form_owner(&self, el: NodeId) -> Option<NodeId> {
        self.doc
            .ancestors(el)
            .find(|&a| self.doc.is_html(a, Name::FORM))
    }

    fn is_submit_button(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if doc.is_html(el, Name::BUTTON) {
            return match doc.attribute_named(el, Name::TYPE) {
                Some(ty) => !ty.eq_ignore_ascii_case("reset") && !ty.eq_ignore_ascii_case("button"),
                None => true,
            };
        }
        matches!(self.input_type(el).as_deref(), Some("submit" | "image"))
    }

    fn is_default(&self, el: NodeId) -> bool {
        let doc = self.doc;
        match self.input_type(el).as_deref() {
            Some("checkbox" | "radio") => return self.has(el, Name::CHECKED),
            _ if doc.is_html(el, Name::OPTION) => return self.has(el, Name::SELECTED),
            _ => {}
        }
        if !self.is_submit_button(el) {
            return false;
        }
        // A form's default button is its first submit button.
        self.form_owner(el).is_some_and(|form| {
            doc.descendants(form)
                .find(|&n| self.is_submit_button(n) && self.form_owner(n) == Some(form))
                == Some(el)
        })
    }

    /// Whether the `required` attribute applies to `el`.
    fn required_applies(&self, el: NodeId) -> bool {
        let doc = self.doc;
        match self.input_type(el).as_deref() {
            Some("hidden" | "range" | "color" | "submit" | "image" | "reset" | "button") => false,
            Some(_) => true,
            None => doc.is_html(el, Name::SELECT) || doc.is_html(el, Name::TEXTAREA),
        }
    }

    /// Whether `el` is an `<input>` of a type the user types text into.
    /// Unknown types count, since they behave as `text`.
    fn is_text_input(&self, el: NodeId) -> bool {
        self.input_type(el).is_some_and(|ty| {
            let not_text = [
                "hidden", "range", "color", "checkbox", "radio", "file", "submit", "image",
                "reset", "button",
            ];
            !not_text.contains(&ty.as_str())
        })
    }

    fn is_read_write(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if self.is_text_input(el) || doc.is_html(el, Name::TEXTAREA) {
            return !self.has(el, Name::READONLY) && !self.is_disabled(el);
        }
        // An editing host or editable content: the nearest contenteditable
        // with a valid value decides.
        std::iter::once(el)
            .chain(doc.ancestors(el))
            .take_while(|&n| doc.is_element(n))
            .find_map(|n| {
                match doc
                    .attribute_named(n, Name::CONTENTEDITABLE)?
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "" | "true" | "plaintext-only" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }
            })
            .unwrap_or(false)
    }

    fn is_placeholder_shown(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if !self.has(el, Name::PLACEHOLDER) {
            return false;
        }
        if doc.is_html(el, Name::TEXTAREA) {
            return doc
                .children(el)
                .all(|c| doc.text(c).is_none_or(str::is_empty));
        }
        self.is_text_input(el)
            && doc
                .attribute_named(el, Name::VALUE)
                .is_none_or(str::is_empty)
    }

    // -----------------------------------------------------------------
    // Language and direction.

    fn matches_lang(&self, ranges: &[Box<str>], el: NodeId) -> bool {
        let doc = self.doc;
        let Some(lang) = std::iter::once(el)
            .chain(doc.ancestors(el))
            .take_while(|&n| doc.is_element(n))
            .find_map(|n| {
                doc.attribute_ns(n, Name::NS_XML, Name::LANG)
                    .or_else(|| doc.attribute_named(n, Name::LANG))
            })
        else {
            return false;
        };
        ranges.iter().any(|range| {
            if &**range == "*" {
                return !lang.is_empty();
            }
            lang.eq_ignore_ascii_case(range)
                || lang.len() > range.len()
                    && lang.as_bytes()[range.len()] == b'-'
                    && lang
                        .get(..range.len())
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(range))
        })
    }

    /// The directionality of `el`, from the nearest `dir` attribute.
    /// `dir=auto` would need the text's bidi class; it is taken as ltr.
    fn is_rtl(&self, el: NodeId) -> bool {
        let doc = self.doc;
        std::iter::once(el)
            .chain(doc.ancestors(el))
            .take_while(|&n| doc.is_element(n))
            .find_map(|n| {
                let dir = doc.attribute_named(n, Name::DIR)?;
                if dir.eq_ignore_ascii_case("rtl") {
                    Some(true)
                } else if dir.eq_ignore_ascii_case("ltr") || dir.eq_ignore_ascii_case("auto") {
                    Some(false)
                } else {
                    None
                }
            })
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------
    // Visibility.

    /// The non-standard `:visible`, decided statically. dasher has no
    /// layout, so an element counts as visible unless the markup alone
    /// hides it: it or an ancestor is never rendered (`<head>`, `<script>`,
    /// `<template>`...), has the `hidden` attribute, is a closed `<dialog>`
    /// or the hidden body of a closed `<details>`, or has `display: none`
    /// in its inline style; or the nearest inline `visibility` is `hidden`
    /// or `collapse`; or it is an `<input type=hidden>`.
    fn is_visible(&self, el: NodeId) -> bool {
        let doc = self.doc;
        if self.input_type(el).as_deref() == Some("hidden") {
            return false;
        }
        let mut visibility_decided = false;
        let mut child: Option<NodeId> = None;
        for n in std::iter::once(el)
            .chain(doc.ancestors(el))
            .take_while(|&n| doc.is_element(n))
        {
            if self.is_html(n) {
                let never_rendered = [
                    Name::HEAD,
                    Name::SCRIPT,
                    Name::STYLE,
                    Name::TEMPLATE,
                    Name::TITLE,
                    Name::META,
                    Name::LINK,
                    Name::BASE,
                    Name::PARAM,
                    Name::SOURCE,
                    Name::TRACK,
                    Name::DATALIST,
                    Name::NOFRAMES,
                ]
                .iter()
                .any(|&name| doc.is_html(n, name));
                if never_rendered
                    || doc.is_html(n, Name::NOSCRIPT) && doc.scripting_enabled()
                    || self.has(n, Name::HIDDEN)
                    || doc.is_html(n, Name::DIALOG) && !self.has(n, Name::OPEN)
                {
                    return false;
                }
                if doc.is_html(n, Name::DETAILS) && !self.has(n, Name::OPEN) {
                    if let Some(child) = child {
                        let summary = doc
                            .element_children(n)
                            .find(|&c| doc.is_html(c, Name::SUMMARY));
                        if summary != Some(child) {
                            return false;
                        }
                    }
                }
            }
            if let Some(style) = doc.attribute_named(n, Name::STYLE) {
                if inline_style(style, "display").is_some_and(|v| v.eq_ignore_ascii_case("none")) {
                    return false;
                }
                if !visibility_decided {
                    if let Some(v) = inline_style(style, "visibility") {
                        visibility_decided = true;
                        if v.eq_ignore_ascii_case("hidden") || v.eq_ignore_ascii_case("collapse") {
                            return false;
                        }
                    }
                }
            }
            child = Some(n);
        }
        true
    }
}

/// The value of the last declaration of `property` in an inline `style`
/// attribute, without `!important`. A rough reading, not a CSS parser: it
/// is only used for the static `:visible`.
fn inline_style<'s>(style: &'s str, property: &str) -> Option<&'s str> {
    style
        .split(';')
        .filter_map(|decl| decl.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case(property))
        .map(|(_, value)| {
            let value = value.trim();
            let important = value.len() >= 10
                && value
                    .get(value.len() - 10..)
                    .is_some_and(|tail| tail.eq_ignore_ascii_case("!important"));
            if important {
                value[..value.len() - 10].trim_end()
            } else {
                value
            }
        })
        .next_back()
}

fn attr_value_matches(op: AttrOp, value: &str, wanted: &str, ignore_case: bool) -> bool {
    let eq = |a: &str, b: &str| {
        if ignore_case {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let n = wanted.len();
    match op {
        AttrOp::Equals => eq(value, wanted),
        AttrOp::Includes => {
            !wanted.is_empty()
                && !wanted.contains(is_ascii_whitespace)
                && value
                    .split(is_ascii_whitespace)
                    .any(|token| eq(token, wanted))
        }
        AttrOp::DashMatch => {
            eq(value, wanted)
                || value.as_bytes().get(n) == Some(&b'-')
                    && value.get(..n).is_some_and(|prefix| eq(prefix, wanted))
        }
        AttrOp::Prefix => n > 0 && value.get(..n).is_some_and(|p| eq(p, wanted)),
        AttrOp::Suffix => {
            n > 0 && value.len() >= n && value.get(value.len() - n..).is_some_and(|s| eq(s, wanted))
        }
        AttrOp::Substring => {
            n > 0
                && if ignore_case {
                    value
                        .as_bytes()
                        .windows(n)
                        .any(|w| w.eq_ignore_ascii_case(wanted.as_bytes()))
                } else {
                    value.contains(wanted)
                }
        }
    }
}

fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_operators() {
        use AttrOp::*;
        let m = |op, v, w| attr_value_matches(op, v, w, false);
        assert!(m(Equals, "a", "a"));
        assert!(!m(Equals, "a", "A"));
        assert!(attr_value_matches(Equals, "a", "A", true));
        assert!(m(Equals, "", ""));

        assert!(m(Includes, "a b\tc", "b"));
        assert!(!m(Includes, "ab", "b"));
        assert!(!m(Includes, "a b", ""));
        assert!(!m(Includes, "a b", "a b"));

        assert!(m(DashMatch, "en", "en"));
        assert!(m(DashMatch, "en-US", "en"));
        assert!(!m(DashMatch, "english", "en"));
        assert!(attr_value_matches(DashMatch, "EN-us", "en", true));

        assert!(m(Prefix, "https://x", "https:"));
        assert!(!m(Prefix, "x", ""));
        assert!(!m(Prefix, "é", "\u{e9}x"));
        assert!(m(Suffix, "a.pdf", ".pdf"));
        assert!(!m(Suffix, "pdf", ""));
        assert!(attr_value_matches(Suffix, "A.PDF", ".pdf", true));
        assert!(m(Substring, "abc", "b"));
        assert!(!m(Substring, "abc", ""));
        assert!(attr_value_matches(Substring, "aBc", "bC", true));
        assert!(!attr_value_matches(Substring, "ab", "abc", true));
    }

    #[test]
    fn reads_inline_styles() {
        assert_eq!(inline_style("display: none", "display"), Some("none"));
        assert_eq!(
            inline_style("color:red; DISPLAY : none !important;", "display"),
            Some("none")
        );
        assert_eq!(
            inline_style("display:none;display:block", "display"),
            Some("block")
        );
        assert_eq!(inline_style("color: red", "display"), None);
    }
}
