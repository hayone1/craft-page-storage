//! The calls to `window.craftStorage`, and the jobs of the Open and Save pages they answer.

use std::rc::Rc;

use craft_cloud::Entry;
use craft_ui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};
use js_sys::{Array, ArrayBuffer, Function, Promise, Reflect, Symbol, Uint8Array};
use serde_json::Value;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use crate::parse;

/// What a page must offer for the Open and Save pages to work.
const REQUIRED: [&str; 5] = ["account", "list", "read", "write", "create"];

/// A page's storage has no account of its own to link.
const NOTHING_TO_LINK: &str = "The page's storage has no account to link.";

const NO_CONVERSATIONS: &str = "The page's storage has nowhere to talk about a document.";

/// Whether the page offers its storage in this shape: `account`, `list`, `read`, `write` and
/// `create` on `window.craftStorage`.
pub fn available() -> bool {
    storage().is_some_and(|s| REQUIRED.iter().all(|name| function(&s, name).is_some()))
}

/// The [`CloudBackend`] for the page's storage. `repaint` asks the app for a frame, so an answer
/// shows up without the user moving the mouse.
pub struct PageStorage {
    repaint: Rc<dyn Fn()>,
}

impl PageStorage {
    pub fn new(repaint: impl Fn() + 'static) -> PageStorage {
        PageStorage { repaint: Rc::new(repaint) }
    }
}

impl CloudBackend for PageStorage {
    fn submit(&self, job: Job, post: Post) {
        let repaint = Rc::clone(&self.repaint);
        spawn_local(async move {
            if let Some(event) = answer(job).await {
                post.send(event);
                repaint();
            }
        });
    }
}

/// The page's answer to one job. `None` for a sign-out, which the pages expect no answer to.
pub async fn answer(job: Job) -> Option<Event> {
    Some(match job {
        Job::Account => Event::Account(account().await),
        Job::SignIn => {
            if let Some(Err(fault)) = call("signIn", &[]).await {
                return Some(Event::Account(Err(fault)));
            }
            Event::Account(account().await)
        }
        Job::SignOut => {
            if let Some(Err(fault)) = call("signOut", &[]).await {
                log(&format!("signOut failed: {fault:?}"));
            }
            return None;
        }
        Job::LinkStart | Job::LinkPoll(_) => Event::Linked(Err(Fault::Refused(NOTHING_TO_LINK.to_owned()))),
        Job::List(path) => {
            let entries = required("list", &[JsValue::from_str(&path)]).await.and_then(|v| entries(&v));
            Event::Listed { path, entries }
        }
        Job::SharedWithMe => Event::Shared(match call("sharedWithMe", &[]).await {
            // A page with no such list has shared nothing: the same answer to a file picker.
            None => Ok(Vec::new()),
            Some(answer) => answer.and_then(|v| entries(&v)),
        }),
        Job::Read { entry, into } => {
            let bytes = match required("read", &[JsValue::from_str(&entry.id)]).await {
                Ok(v) => bytes(v).await,
                Err(fault) => Err(fault),
            };
            Event::Read { entry, into, bytes }
        }
        Job::Write { uid, entry, bytes } => {
            let if_match = entry.etag.as_deref().map_or(JsValue::UNDEFINED, JsValue::from_str);
            let args = [JsValue::from_str(&entry.id), Uint8Array::from(bytes.as_slice()).into(), if_match];
            Event::Saved { uid, entry: required("write", &args).await.and_then(|v| stored(&v)) }
        }
        Job::Create { uid, folder, name, bytes } => {
            let args = [JsValue::from_str(&folder), JsValue::from_str(&name), Uint8Array::from(bytes.as_slice()).into()];
            Event::Saved { uid, entry: required("create", &args).await.and_then(|v| stored(&v)) }
        }
        Job::Conversation(_) => Event::Conversation(Err(Fault::Refused(NO_CONVERSATIONS.to_owned()))),
    })
}

async fn account() -> Result<Account, Fault> {
    let shared = storage().is_some_and(|s| function(&s, "sharedWithMe").is_some());
    let v = required("account", &[]).await?;
    parse::account(&json(&v)?, shared).map_err(malformed)
}

fn entries(v: &JsValue) -> Result<Vec<Entry>, Fault> {
    parse::entries(&json(v)?).map_err(malformed)
}

/// The file a write or a create stored, as the page describes it now.
fn stored(v: &JsValue) -> Result<Entry, Fault> {
    parse::entry(&json(v)?).map_err(malformed)
}

/// A file's bytes: a `Uint8Array` (or another view), an `ArrayBuffer`, or a `Blob` or `File`.
/// Checked without `instanceof`, so values from another frame (a page that embeds the app) pass.
async fn bytes(v: JsValue) -> Result<Vec<u8>, Fault> {
    let v = match function(&v, "arrayBuffer") {
        Some(read) => settle(read.call0(&v)).await?,
        None => v,
    };
    if ArrayBuffer::is_view(&v) {
        let field = |key: &str| Reflect::get(&v, &JsValue::from_str(key)).ok();
        let (buffer, offset, length) = (field("buffer"), field("byteOffset").and_then(|n| n.as_f64()), field("byteLength").and_then(|n| n.as_f64()));
        if let (Some(buffer), Some(offset), Some(length)) = (buffer, offset, length) {
            return Ok(Uint8Array::new_with_byte_offset_and_length(&buffer, offset as u32, length as u32).to_vec());
        }
    }
    let tag = Reflect::get(&v, &Symbol::to_string_tag()).ok().and_then(|t| t.as_string());
    if tag.as_deref() == Some("ArrayBuffer") {
        return Ok(Uint8Array::new(&v).to_vec());
    }
    Err(malformed("file contents that are not a Uint8Array, an ArrayBuffer or a Blob"))
}

/// `window.craftStorage`, while the page offers it.
fn storage() -> Option<JsValue> {
    Reflect::get(&js_sys::global(), &JsValue::from_str("craftStorage")).ok().filter(JsValue::is_object)
}

fn function(on: &JsValue, name: &str) -> Option<Function> {
    if !on.is_object() {
        return None;
    }
    Reflect::get(on, &JsValue::from_str(name)).ok()?.dyn_into().ok()
}

/// Call one of the page's functions and wait for its answer. `None` when the page has no such
/// function, for the optional ones.
async fn call(name: &str, args: &[JsValue]) -> Option<Result<JsValue, Fault>> {
    let Some(storage) = storage() else {
        return Some(Err(Fault::Unreachable("The page no longer offers its storage (window.craftStorage).".to_owned())));
    };
    let f = function(&storage, name)?;
    let args: Array = args.iter().collect();
    Some(settle(f.apply(&storage, &args)).await)
}

async fn required(name: &str, args: &[JsValue]) -> Result<JsValue, Fault> {
    call(name, args).await.unwrap_or_else(|| Err(Fault::Refused(format!("The page's storage has no {name}()."))))
}

/// A call's value, or the promise it returned once that settles. A throw and a rejection are the
/// same failure.
async fn settle(called: Result<JsValue, JsValue>) -> Result<JsValue, Fault> {
    let answer = called.map_err(|e| rejected(&e))?;
    JsFuture::from(Promise::resolve(&answer)).await.map_err(|e| rejected(&e))
}

/// What a throw or a rejection said: an `Error` or an object with `code` and `message`, or text.
fn rejected(e: &JsValue) -> Fault {
    if let Some(text) = e.as_string() {
        return parse::fault(None, Some(text));
    }
    let field = |key: &str| if e.is_object() { Reflect::get(e, &JsValue::from_str(key)).ok().and_then(|v| v.as_string()) } else { None };
    parse::fault(field("code").as_deref(), field("message"))
}

fn json(v: &JsValue) -> Result<Value, Fault> {
    let text = js_sys::JSON::stringify(v).ok().and_then(|s| s.as_string()).ok_or_else(|| malformed("an answer with no JSON"))?;
    serde_json::from_str(&text).map_err(malformed)
}

/// An answer the contract does not allow.
fn malformed(detail: impl std::fmt::Display) -> Fault {
    Fault::Refused(format!("The page's storage gave an answer that could not be read: {detail}"))
}

fn log(text: &str) {
    let console = Reflect::get(&js_sys::global(), &JsValue::from_str("console")).ok();
    if let Some(warn) = console.as_ref().and_then(|c| function(c, "warn")) {
        let _ = warn.call1(&JsValue::NULL, &JsValue::from_str(&format!("craft-page-storage: {text}")));
    }
}
