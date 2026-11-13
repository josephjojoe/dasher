//! The native members for attributes: `getAttribute` and friends on
//! `Element`, and the attributes HTML elements reflect as IDL attributes
//! (`a.href`, `input.type`, `el.hidden`...).
//!
//! The members are entries in `MEMBERS` (see the `members` module). Reads
//! run with the document borrowed; changes go through the mutation natives
//! in `natives` (`set_attr` and friends), which record what
//! `MutationObserver`s and custom element reactions need and count it as
//! DOM activity, exactly as the JS glue's calls did.
//!
//! # Reflected attributes
//!
//! `reflect()` in `dom.js` describes each reflected IDL attribute by a type
//! and an attribute name. It hands each description to `N.reflect`
//! ([`install_reflect`]), which makes the getter and setter native C
//! functions: their magic number is the description's index in the
//! per-context table `Members::reflected`, so one C function,
//! [`call_reflected`], serves them all. A description that needs JS (a
//! fallback computed by a function) keeps its JS accessor.

use std::borrow::Cow;
use std::ffi::{CString, c_int};

use dasher_dom::{Document, Name, NodeId};
use rquickjs::{Ctx, Exception, Object, Result, Value, qjs};

use super::members::{
    Call, JsStr, Members, Outcome, Ret, Thrown, Write, ctx_of, new_string, throw_type,
};
use super::natives::{
    html_lower, remove_attr, remove_attr_ns, set_attr, set_attr_ns, valid_attribute_name,
};

type Done = std::result::Result<Ret<'static>, Thrown>;

/// Argument `i` as `nullableStr(v) || null` does in `dom.js`: `None` for
/// `null`, `undefined`, a missing argument or the empty string.
fn namespace_arg(w: &Write<'_>, i: usize) -> std::result::Result<Option<String>, Thrown> {
    let v = w.arg(i);
    // SAFETY: tag checks of a live value.
    if unsafe { qjs::JS_IsNull(v) || qjs::JS_IsUndefined(v) } {
        return Ok(None);
    }
    let s = w.str_arg(i)?;
    let s = s.as_str();
    Ok((!s.is_empty()).then(|| s.into_owned()))
}

/// The value of the attribute `getAttribute(name)` finds.
fn attribute<'d>(doc: &'d Document, n: NodeId, name: &str) -> Option<&'d str> {
    doc.attribute(n, &html_lower(doc, n, name))
}

pub(super) fn get_attribute<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let name = c.str_arg(0)?;
    Ok(attribute(&c.doc, n, &name.as_str()).map_or(Ret::Null, |v| Ret::Str(Cow::Borrowed(v))))
}

pub(super) fn has_attribute<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let name = c.str_arg(0)?;
    Ok(Ret::Bool(attribute(&c.doc, n, &name.as_str()).is_some()))
}

pub(super) fn has_attributes<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Bool(!c.doc.attrs(n).is_empty()))
}

/// The attribute's qualified name: the cached local name, or `prefix:local`.
pub(super) fn qualified_name(
    ctx: *mut qjs::JSContext,
    members: &Members,
    doc: &Document,
    attr: &dasher_dom::Attr,
) -> qjs::JSValue {
    let prefix = doc.attr_prefix(attr);
    if prefix == Name::EMPTY {
        return members.name(ctx, doc, attr.local(), false);
    }
    let name = format!("{}:{}", doc.name_str(prefix), doc.name_str(attr.local()));
    new_string(ctx, &name)
}

pub(super) fn get_attribute_names<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    // SAFETY: `c.ctx` is the running call's context.
    let array = unsafe { qjs::JS_NewArray(c.ctx) };
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(array) } {
        return Err(Thrown::Pending);
    }
    for (i, attr) in c.doc.attrs(n).iter().enumerate() {
        let name = qualified_name(c.ctx, c.members, &c.doc, attr);
        // SAFETY: `array` is ours; the call takes `name`'s reference.
        if unsafe { qjs::JS_IsException(name) }
            || unsafe { qjs::JS_SetPropertyUint32(c.ctx, array, i as u32, name) } < 0
        {
            // SAFETY: `array` is ours.
            unsafe { qjs::JS_FreeValue(c.ctx, array) };
            return Err(Thrown::Pending);
        }
    }
    Ok(Ret::Value(array))
}

/// `getAttributeNS`'s lookup.
fn attribute_ns<'d>(
    doc: &'d Document,
    n: NodeId,
    ns: Option<&str>,
    local: &str,
) -> Option<&'d str> {
    let names = doc.names();
    let (ns, local) = (names.get(ns.unwrap_or(""))?, names.get(local)?);
    doc.attribute_ns(n, ns, local)
}

pub(super) fn get_attribute_ns(w: &Write<'_>) -> Done {
    let ns = namespace_arg(w, 0)?;
    let local = w.str_arg(1)?;
    let doc = w.dom.doc();
    Ok(
        attribute_ns(&doc, w.id, ns.as_deref(), &local.as_str())
            .map_or(Ret::Null, |v| Ret::Str(Cow::Owned(v.to_owned()))),
    )
}

pub(super) fn has_attribute_ns(w: &Write<'_>) -> Done {
    let ns = namespace_arg(w, 0)?;
    let local = w.str_arg(1)?;
    let doc = w.dom.doc();
    Ok(Ret::Bool(
        attribute_ns(&doc, w.id, ns.as_deref(), &local.as_str()).is_some(),
    ))
}

fn invalid_name(name: &str) -> Thrown {
    Thrown::Dom(
        "InvalidCharacterError",
        format!("'{name}' is not a valid attribute name."),
    )
}

pub(super) fn set_attribute(w: &Write<'_>) -> Done {
    let name = w.str_arg(0)?;
    let value = w.str_arg(1)?;
    let name = name.as_str();
    if !valid_attribute_name(&name) {
        return Err(invalid_name(&name));
    }
    set_attr(&w.rctx(), w.dom, w.id, &name, &value.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn set_attribute_ns(w: &Write<'_>) -> Done {
    let ns = namespace_arg(w, 0)?;
    let name = w.str_arg(1)?;
    let value = w.str_arg(2)?;
    let name = name.as_str();
    if !valid_attribute_name(&name) {
        return Err(invalid_name(&name));
    }
    let ns = ns.unwrap_or_default();
    if ns.is_empty() && name.contains(':') {
        return Err(Thrown::Dom(
            "NamespaceError",
            "The namespace URI provided ('') is not valid for the qualified name provided."
                .to_owned(),
        ));
    }
    set_attr_ns(&w.rctx(), w.dom, w.id, &ns, &name, &value.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn remove_attribute(w: &Write<'_>) -> Done {
    let name = w.str_arg(0)?;
    remove_attr(&w.rctx(), w.dom, w.id, &name.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn remove_attribute_ns(w: &Write<'_>) -> Done {
    let ns = namespace_arg(w, 0)?;
    let local = w.str_arg(1)?;
    remove_attr_ns(&w.rctx(), w.dom, w.id, ns.as_deref(), &local.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn toggle_attribute(w: &Write<'_>) -> Done {
    let name = w.str_arg(0)?;
    let name = name.as_str();
    // Only the boolean values count, as `force === true` in the glue.
    let force = w.arg(1);
    let present = attribute(&w.dom.doc(), w.id, &name).is_some();
    if present {
        if force == qjs::JS_TRUE {
            return Ok(Ret::Bool(true));
        }
        remove_attr(&w.rctx(), w.dom, w.id, &name)?;
        return Ok(Ret::Bool(false));
    }
    if force == qjs::JS_FALSE {
        return Ok(Ret::Bool(false));
    }
    if !valid_attribute_name(&name) {
        return Err(invalid_name(&name));
    }
    set_attr(&w.rctx(), w.dom, w.id, &name, "")?;
    Ok(Ret::Bool(true))
}

/// The setter of an IDL attribute that reflects `name` as a string.
fn set_string(w: &Write<'_>, name: &str) -> Done {
    let value = w.str_arg(0)?;
    set_attr(&w.rctx(), w.dom, w.id, name, &value.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn set_id(w: &Write<'_>) -> Done {
    set_string(w, "id")
}

pub(super) fn set_class_name(w: &Write<'_>) -> Done {
    set_string(w, "class")
}

// -------------------------------------------------------------------------
// Reflected IDL attributes.

/// How an IDL attribute reflects its content attribute (`reflect()`'s
/// types in `dom.js`).
enum Reflect {
    /// The value, or `missing` without the attribute.
    String { missing: String },
    /// The value or `null`; setting `null` removes the attribute.
    Nullable,
    /// The value resolved against the document's base URL, or `""`.
    Url,
    /// Whether the attribute is present.
    Bool,
    /// `parseInt(value, 10)`, or `fallback` if that fails (or is negative,
    /// for `unsigned`).
    Long { unsigned: bool, fallback: f64 },
    /// `parseFloat(value)` if finite, else `fallback`.
    Number { fallback: Option<f64> },
    /// The value lower-cased if it is one of `values`, `missing` without
    /// the attribute, `invalid` otherwise.
    Enum {
        values: Vec<String>,
        missing: String,
        invalid: String,
    },
}

/// One reflected IDL attribute: its content attribute and how it reflects.
pub(crate) struct Reflected {
    attr: String,
    how: Reflect,
}

/// Makes the IDL attributes `reflect()` in `dom.js` described native:
/// `list` holds `[proto, prop, type, attr, extra]` for each, and `proto[prop]`
/// becomes a native accessor reflecting `attr` as `type` says, in place of
/// the JS one `reflect()` defined. Descriptions that need JS (a fallback
/// computed by a function) keep their JS accessor. Called once `members`
/// is attached to the context.
pub(super) fn install_reflected<'js>(
    ctx: &Ctx<'js>,
    members: &Members,
    list: rquickjs::Array<'js>,
) -> Result<()> {
    let object: Object<'js> = ctx.globals().get("Object")?;
    let define: rquickjs::Function<'js> = object.get("defineProperty")?;
    for entry in list.iter::<rquickjs::Array<'js>>() {
        let entry = entry?;
        let proto: Object<'js> = entry.get(0)?;
        let prop: String = entry.get(1)?;
        let kind: String = entry.get(2)?;
        let attr: String = entry.get(3)?;
        let extra: Value<'js> = entry.get(4)?;
        let Some(how) = reflect_kind(&kind, &extra)? else {
            continue;
        };
        let index = members.add_reflected(Reflected { attr, how });
        let getter = reflect_function(ctx, &format!("get {prop}"), 0, index * 2)?;
        let setter = reflect_function(ctx, &format!("set {prop}"), 1, index * 2 + 1)?;
        let desc = Object::new(ctx.clone())?;
        desc.set("get", getter)?;
        desc.set("set", setter)?;
        desc.set("enumerable", true)?;
        desc.set("configurable", true)?;
        define.call::<_, Value<'js>>((proto, prop, desc))?;
    }
    Ok(())
}

/// What `reflect()`'s `type` and `extra` mean, or `None` if it needs JS.
fn reflect_kind(kind: &str, extra: &Value<'_>) -> Result<Option<Reflect>> {
    let number = |extra: &Value<'_>| extra.as_number();
    Ok(Some(match kind {
        "string" => Reflect::String {
            missing: match extra.as_string() {
                Some(s) => s.to_string()?,
                None if extra.is_undefined() => String::new(),
                None => return Ok(None),
            },
        },
        "nullable" => Reflect::Nullable,
        "url" => Reflect::Url,
        "bool" => Reflect::Bool,
        "long" | "ulong" => Reflect::Long {
            unsigned: kind == "ulong",
            fallback: if extra.is_undefined() {
                0.0
            } else {
                match number(extra) {
                    Some(n) => n,
                    None => return Ok(None),
                }
            },
        },
        "number" => Reflect::Number {
            fallback: if extra.is_undefined() {
                None
            } else {
                match number(extra) {
                    Some(n) => Some(n),
                    None => return Ok(None),
                }
            },
        },
        "enum" => {
            let Some(spec) = extra.as_array() else {
                return Ok(None);
            };
            let values: Vec<String> = spec.get(0)?;
            let missing: Option<String> = spec.get(1)?;
            let missing = missing.unwrap_or_default();
            let invalid: Option<String> = spec.get(2)?;
            Reflect::Enum {
                values,
                invalid: invalid.unwrap_or_else(|| missing.clone()),
                missing,
            }
        }
        _ => return Ok(None),
    }))
}

/// A C function for a reflected attribute's getter (even `magic`) or
/// setter (odd), named and sized as the JS accessor was.
fn reflect_function<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    length: c_int,
    magic: usize,
) -> Result<Value<'js>> {
    let name = CString::new(name).expect("property names have no NUL");
    let magic = c_int::try_from(magic).map_err(|_| Exception::throw_internal(ctx, "too many reflected attributes"))?;
    super::members::new_magic_function(ctx, call_reflected, &name, length, magic)
}

/// The C function of every reflected attribute's getter and setter (see
/// the module docs).
unsafe extern "C" fn call_reflected(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    // SAFETY: see `members::call_member`, which this follows.
    let members = unsafe { qjs::JS_GetContextOpaque(ctx).cast::<Members>().as_ref() };
    let Some(members) = members else {
        // SAFETY: QuickJS passes the live context of the call.
        return unsafe { throw_type(ctx, c"Illegal invocation") };
    };
    // SAFETY: `this` is alive for the call.
    let Some(wrapper) = (unsafe { members.wrapper(ctx, this) }) else {
        // SAFETY: as above.
        return unsafe { Thrown::Illegal.throw(ctx) };
    };
    let Some(id) = wrapper.id() else {
        // SAFETY: as above.
        return unsafe { Thrown::Stale.throw(ctx) };
    };
    let dom = wrapper.dom();
    let index = (magic / 2) as usize;
    let reflected = members.reflected(index);
    if magic % 2 == 0 {
        let doc = dom.doc();
        if !doc.is_element(id) {
            drop(doc);
            // SAFETY: as above.
            return unsafe { Thrown::Illegal.throw(ctx) };
        }
        // Reflected attribute names are lower case already.
        let value = doc.attribute(id, &reflected.attr);
        if let (Reflect::Url, Some(value)) = (&reflected.how, value) {
            // Finding the base URL runs JS: let go of the document first.
            let value = value.to_owned();
            drop(doc);
            return members.resolve_url(ctx, &value);
        }
        return get_reflected(ctx, members, reflected, value);
    }
    if !dom.doc().is_element(id) {
        // SAFETY: as above.
        return unsafe { Thrown::Illegal.throw(ctx) };
    }
    let value = if argc > 0 && !argv.is_null() {
        // SAFETY: QuickJS passes `argc` live values at `argv`.
        unsafe { *argv }
    } else {
        qjs::JS_UNDEFINED
    };
    // SAFETY: as above.
    let rctx = unsafe { ctx_of(ctx) };
    match set_reflected(ctx, &rctx, dom, id, reflected, value) {
        Ok(()) => qjs::JS_UNDEFINED,
        // SAFETY: as above.
        Err(thrown) => unsafe { thrown.throw(ctx) },
    }
}

fn get_reflected(
    ctx: *mut qjs::JSContext,
    members: &Members,
    reflected: &Reflected,
    value: Option<&str>,
) -> qjs::JSValue {
    match &reflected.how {
        Reflect::String { missing } => new_string(ctx, value.unwrap_or(missing)),
        Reflect::Nullable => value.map_or(qjs::JS_NULL, |v| new_string(ctx, v)),
        // A present value is resolved by the caller.
        Reflect::Url => members.fixed(ctx, ""),
        Reflect::Bool => {
            if value.is_some() {
                qjs::JS_TRUE
            } else {
                qjs::JS_FALSE
            }
        }
        Reflect::Long { unsigned, fallback } => {
            let n = value.and_then(parse_int).filter(|&n| !*unsigned || n >= 0.0);
            number(n.unwrap_or(*fallback))
        }
        Reflect::Number { fallback } => {
            match value.and_then(parse_float).filter(|n| n.is_finite()) {
                Some(n) => number(n),
                None => fallback.map_or(qjs::JS_UNDEFINED, number),
            }
        }
        Reflect::Enum {
            values,
            missing,
            invalid,
        } => {
            let Some(v) = value else {
                return new_string(ctx, missing);
            };
            let lower = v.to_lowercase();
            match values.iter().find(|x| **x == lower) {
                Some(x) => new_string(ctx, x),
                None => new_string(ctx, invalid),
            }
        }
    }
}

/// A JS number: an int when it is one.
fn number(n: f64) -> qjs::JSValue {
    if n.fract() == 0.0 && n >= f64::from(i32::MIN) && n <= f64::from(i32::MAX) && !(n == 0.0 && n.is_sign_negative()) {
        qjs::JS_MKVAL(qjs::JS_TAG_INT, n as i32)
    } else {
        qjs::JS_NewFloat64(n)
    }
}

fn set_reflected(
    ctx: *mut qjs::JSContext,
    rctx: &Ctx<'_>,
    dom: &std::rc::Rc<super::DomState>,
    id: NodeId,
    reflected: &Reflected,
    value: qjs::JSValue,
) -> std::result::Result<(), Thrown> {
    let attr = reflected.attr.as_str();
    let text: Cow<'_, str> = match &reflected.how {
        Reflect::Bool => {
            // SAFETY: `ToBoolean` of a live value runs no JS.
            if unsafe { qjs::JS_ToBool(ctx, value) } > 0 {
                Cow::Borrowed("")
            } else {
                remove_attr(rctx, dom, id, attr)?;
                return Ok(());
            }
        }
        // SAFETY: a tag check.
        Reflect::Nullable if unsafe { qjs::JS_IsNull(value) } => {
            remove_attr(rctx, dom, id, attr)?;
            return Ok(());
        }
        Reflect::Long { unsigned, .. } => {
            let mut n = 0i32;
            // SAFETY: `ToInt32` of a live value; it may run JS.
            if unsafe { qjs::JS_ToInt32(ctx, &mut n, value) } < 0 {
                return Err(Thrown::Pending);
            }
            Cow::Owned(if *unsigned {
                (n as u32).to_string()
            } else {
                n.to_string()
            })
        }
        Reflect::Number { .. } => {
            // `String(+value)`.
            let mut n = 0f64;
            // SAFETY: `ToNumber` of a live value; it may run JS.
            if unsafe { qjs::JS_ToFloat64(ctx, &mut n, value) } < 0 {
                return Err(Thrown::Pending);
            }
            let s = number(n);
            // SAFETY: `s` is a number, so converting it runs no JS.
            let s = unsafe { JsStr::new(ctx, s) }?;
            Cow::Owned(s.as_str().into_owned())
        }
        _ => {
            // SAFETY: `value` is the call's argument, alive.
            let s = unsafe { JsStr::new(ctx, value) }?;
            Cow::Owned(s.as_str().into_owned())
        }
    };
    set_attr(rctx, dom, id, attr, &text)?;
    Ok(())
}

/// `parseInt(s, 10)`: leading whitespace, a sign, then decimal digits.
fn parse_int(s: &str) -> Option<f64> {
    let s = s.trim_start_matches(is_js_space);
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    let n: f64 = digits[..end].parse().ok()?;
    Some(if neg { -n } else { n })
}

/// `parseFloat(s)`: the longest prefix (after whitespace) that is a decimal
/// literal, or `Infinity`.
fn parse_float(s: &str) -> Option<f64> {
    let s = s.trim_start_matches(is_js_space);
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    if s[i..].starts_with("Infinity") {
        return Some(if b[0] == b'-' { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    let mut any = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        let mut j = i + 1;
        if digits(&mut j) || any {
            any = true;
            i = j;
        }
    }
    if !any {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if digits(&mut j) {
            i = j;
        }
    }
    s[..i].parse().ok()
}

/// JS's `WhiteSpace` and `LineTerminator`, which `parseInt` and
/// `parseFloat` skip.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_float, parse_int};

    #[test]
    fn parses_as_js_does() {
        assert_eq!(parse_int(" 12px"), Some(12.0));
        assert_eq!(parse_int("-3"), Some(-3.0));
        assert_eq!(parse_int("+7"), Some(7.0));
        assert_eq!(parse_int("x1"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_float("1.5e3x"), Some(1500.0));
        assert_eq!(parse_float(".5"), Some(0.5));
        assert_eq!(parse_float("5."), Some(5.0));
        assert_eq!(parse_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_float("1e"), Some(1.0));
        assert_eq!(parse_float("."), None);
        assert_eq!(parse_float("abc"), None);
    }
}
