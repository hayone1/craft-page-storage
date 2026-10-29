#![cfg(target_arch = "wasm32")]

//! The backend against a fake page, in Node (`wasm-bindgen-test-runner`): the crate reads
//! `globalThis.craftStorage`, which is `window` in a page.

use craft_cloud::{Capability, Entry, EntryKind};
use craft_page_storage::{PageStorage, answer, available};
use craft_ui::cloud::{self, CloudApp, CloudFiles, Destination, DocumentKind, Event, Fault, Job};
use craft_ui::presence::PresenceBar;
use craft_ui::share::ShareDialog;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

// A fake page with one file, `home/a.txt` (bytes 1, 2, 3, etag "1"). `globalThis.readAs` picks
// the form `read` answers in, `globalThis.signedOut` makes `account` reject, and every call goes
// to a log.
#[wasm_bindgen(inline_js = r#"
const fail = (code, message) => Object.assign(new Error(message), { code });
export function install() {
  const files = new Map([["home/a.txt", { bytes: new Uint8Array([1, 2, 3]), etag: "1" }]]);
  const entry = (id) => ({ id, path: id, size: files.get(id).bytes.length, etag: files.get(id).etag });
  const log = [];
  globalThis.craftLog = log;
  globalThis.readAs = "u8";
  globalThis.signedOut = false;
  globalThis.bump = (id) => { files.get(id).etag += "+"; };
  globalThis.craftStorage = {
    account() {
      log.push(["account"]);
      if (globalThis.signedOut) throw fail("unauthenticated", "Sign in first");
      return { display: "Ada", capabilities: ["etags", "sharing", "contacts"] };
    },
    async list(path) {
      log.push(["list", path]);
      if (path === "broken") return { id: "not a list" };
      const inside = [...files.keys()].filter((id) => id.startsWith(path)).map(entry);
      return [{ id: "home/Plans", name: "Plans", kind: "folder" }, ...inside];
    },
    async read(id) {
      log.push(["read", id]);
      const bytes = files.get(id).bytes;
      switch (globalThis.readAs) {
        case "buffer": return bytes.slice().buffer;
        case "blob": return new Blob([bytes]);
        case "view": return new Uint8Array([0, ...bytes, 0]).subarray(1, 4);
        case "text": return "1,2,3";
        case "throw": throw "the disk is gone";
        default: return bytes;
      }
    },
    async write(id, bytes, ifMatch) {
      log.push(["write", id, Array.from(bytes), ifMatch ?? null]);
      const file = files.get(id);
      if (ifMatch !== undefined && file.etag !== ifMatch) throw fail("conflict", "Somebody saved first");
      files.set(id, { bytes: bytes.slice(), etag: String(Number.parseInt(file.etag, 10) + 1) });
      return entry(id);
    },
    create(folder, name, bytes) {
      log.push(["create", folder, name, Array.from(bytes)]);
      if (name === "sync") throw fail("forbidden", "Read only here");
      const id = folder ? `${folder}/${name}` : name;
      files.set(id, { bytes: bytes.slice(), etag: "1" });
      return Promise.resolve(entry(id));
    },
  };
}
export function add_shared() {
  globalThis.craftStorage.sharedWithMe = async () => [{ id: "team/b.txt", sharedBy: "Grace", writable: false }];
}
export function add_sign_in() {
  globalThis.craftStorage.signIn = async () => { globalThis.craftLog.push(["signIn"]); globalThis.signedOut = false; };
  globalThis.craftStorage.signOut = async () => { globalThis.craftLog.push(["signOut"]); globalThis.signedOut = true; };
}
export function set(key, value) { globalThis[key] = value; }
export function old_host() { globalThis.craftStorage = { chooseOpen() {}, chooseSave() {}, write() {} }; }
export function no_host() { delete globalThis.craftStorage; }
export function log() { return JSON.stringify(globalThis.craftLog); }
export function bump(id) { globalThis.bump(id); }
export function sleep(ms) { return new Promise((r) => setTimeout(r, ms)); }
"#)]
extern "C" {
    fn install();
    fn add_shared();
    fn add_sign_in();
    fn set(key: &str, value: JsValue);
    fn old_host();
    fn no_host();
    fn log() -> String;
    fn bump(id: &str);
    fn sleep(ms: u32) -> js_sys::Promise;
}

async fn wait(ms: u32) {
    JsFuture::from(sleep(ms)).await.unwrap();
}

fn file(id: &str, etag: Option<&str>) -> Entry {
    Entry {
        id: id.into(),
        room: id.into(),
        name: id.rsplit('/').next().unwrap().into(),
        path: id.into(),
        kind: EntryKind::File,
        size: None,
        modified: None,
        mime: None,
        etag: etag.map(Into::into),
        writable: true,
        shared_by: None,
    }
}

async fn saved(job: Job) -> Result<Entry, Fault> {
    let Some(Event::Saved { entry, .. }) = answer(job).await else { panic!("not a save answer") };
    entry
}

async fn read(id: &str) -> Result<Vec<u8>, Fault> {
    let Some(Event::Read { bytes, .. }) = answer(Job::Read { entry: file(id, None), into: None }).await else { panic!("not a read answer") };
    bytes
}

#[wasm_bindgen_test]
fn only_a_page_with_the_whole_contract_is_offered() {
    no_host();
    assert!(!available());
    // The older shape (the page's own Open and Save dialogs) is not this contract.
    old_host();
    assert!(!available());
    install();
    assert!(available());
}

#[wasm_bindgen_test]
async fn the_account_keeps_only_what_this_backend_serves() {
    install();
    let Some(Event::Account(Ok(account))) = answer(Job::Account).await else { panic!("no account") };
    assert_eq!(account.display, "Ada");
    assert_eq!(account.capabilities, [Capability::Files, Capability::Etags]);
    assert!(account.provider_linked);
    add_shared();
    let Some(Event::Account(Ok(account))) = answer(Job::Account).await else { panic!("no account") };
    assert_eq!(account.capabilities, [Capability::Files, Capability::SharedWithMe, Capability::Etags]);
}

#[wasm_bindgen_test]
async fn nobody_signed_in_is_asked_to_sign_in() {
    install();
    set("signedOut", JsValue::TRUE);
    assert!(matches!(answer(Job::Account).await, Some(Event::Account(Err(Fault::Unauthenticated)))));
    add_sign_in();
    assert!(matches!(answer(Job::SignIn).await, Some(Event::Account(Ok(_)))));
    assert!(answer(Job::SignOut).await.is_none());
    assert!(log().contains(r#"["signIn"],["account"],["signOut"]"#), "{}", log());
}

#[wasm_bindgen_test]
async fn a_folder_lists_its_files_and_folders() {
    install();
    let Some(Event::Listed { path, entries: Ok(entries) }) = answer(Job::List("home".into())).await else { panic!("no listing") };
    assert_eq!(path, "home");
    assert_eq!(entries.len(), 2);
    assert_eq!((entries[0].name.as_str(), entries[0].kind, entries[0].path.as_str()), ("Plans", EntryKind::Folder, "home/Plans"));
    assert_eq!((entries[1].name.as_str(), entries[1].size, entries[1].etag.as_deref()), ("a.txt", Some(3), Some("1")));
    let Some(Event::Listed { entries, .. }) = answer(Job::List("broken".into())).await else { panic!("no listing") };
    assert!(matches!(entries, Err(Fault::Refused(_))));
}

#[wasm_bindgen_test]
async fn shared_files_come_from_shared_with_me_when_the_page_has_it() {
    install();
    assert!(matches!(answer(Job::SharedWithMe).await, Some(Event::Shared(Ok(e))) if e.is_empty()));
    add_shared();
    let Some(Event::Shared(Ok(shared))) = answer(Job::SharedWithMe).await else { panic!("no shared list") };
    assert_eq!((shared[0].name.as_str(), shared[0].shared_by.as_deref(), shared[0].writable), ("b.txt", Some("Grace"), false));
}

#[wasm_bindgen_test]
async fn a_read_takes_every_form_of_bytes() {
    install();
    for form in ["u8", "buffer", "blob", "view"] {
        set("readAs", form.into());
        assert_eq!(read("home/a.txt").await, Ok(vec![1, 2, 3]), "{form}");
    }
    set("readAs", "text".into());
    assert!(matches!(read("home/a.txt").await, Err(Fault::Refused(_))));
    set("readAs", "throw".into());
    assert_eq!(read("home/a.txt").await, Err(Fault::Refused("the disk is gone".into())));
}

#[wasm_bindgen_test]
async fn a_save_sends_the_version_it_read_and_keeps_the_new_one() {
    install();
    let stored = saved(Job::Write { uid: 1, entry: file("home/a.txt", Some("1")), bytes: vec![9] }).await.unwrap();
    assert_eq!(stored.etag.as_deref(), Some("2"));
    assert!(log().contains(r#"["write","home/a.txt",[9],"1"]"#), "{}", log());
    // No etag: write regardless, as a page without etags does.
    saved(Job::Write { uid: 1, entry: file("home/a.txt", None), bytes: vec![8] }).await.unwrap();
    assert!(log().contains(r#"["write","home/a.txt",[8],null]"#), "{}", log());
}

#[wasm_bindgen_test]
async fn a_save_over_newer_work_is_a_conflict() {
    install();
    bump("home/a.txt");
    assert_eq!(saved(Job::Write { uid: 1, entry: file("home/a.txt", Some("1")), bytes: vec![9] }).await, Err(Fault::Conflict));
}

#[wasm_bindgen_test]
async fn save_as_creates_a_file() {
    install();
    let stored = saved(Job::Create { uid: 2, folder: "home".into(), name: "b.txt".into(), bytes: vec![5] }).await.unwrap();
    assert_eq!((stored.id.as_str(), stored.name.as_str()), ("home/b.txt", "b.txt"));
    // A throw before any promise is the same failure as a rejection.
    let refused = saved(Job::Create { uid: 2, folder: "home".into(), name: "sync".into(), bytes: vec![5] }).await;
    assert_eq!(refused, Err(Fault::Refused("Read only here".into())));
}

#[wasm_bindgen_test]
async fn a_page_that_took_its_storage_away_is_unreachable() {
    install();
    no_host();
    assert!(matches!(read("home/a.txt").await, Err(Fault::Unreachable(_))));
}

#[wasm_bindgen_test]
async fn linking_and_conversations_are_refused() {
    install();
    assert!(matches!(answer(Job::LinkStart).await, Some(Event::Linked(Err(Fault::Refused(_))))));
    assert!(matches!(answer(Job::Conversation("home/a.txt".into())).await, Some(Event::Conversation(Err(Fault::Refused(_))))));
}

// The real Open and Save pages, driven by an app that holds one document.

struct Notebook {
    cloud: CloudFiles,
    share: ShareDialog,
    presence: PresenceBar,
    opened: Vec<(String, Vec<u8>)>,
    saved: Vec<u64>,
    said: Vec<String>,
}

impl CloudApp for Notebook {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }
    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }
    fn kind(&self) -> DocumentKind {
        DocumentKind { extensions: &["txt"], mimes: &["text/plain"], noun: "note", default_extension: "txt" }
    }
    fn active(&self) -> Option<u64> {
        Some(1)
    }
    fn title(&self, _uid: u64) -> String {
        "a.txt".into()
    }
    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        self.opened.push((name.into(), bytes));
        Ok(Some(1))
    }
    fn encode(&mut self, _uid: u64, _into: Destination<'_>) -> Result<Vec<u8>, String> {
        Ok(vec![7, 7])
    }
    fn close(&mut self, _uid: u64) {}
    fn mark_saved(&mut self, uid: u64) {
        self.saved.push(uid);
    }
    fn sharing(&mut self) -> &mut ShareDialog {
        &mut self.share
    }
    fn presence(&mut self) -> &mut PresenceBar {
        &mut self.presence
    }
    fn paint_icon(&self, _painter: &egui::Painter, _rect: egui::Rect, _kind: EntryKind, _colour: egui::Color32) {}
    fn open_url(&self, _url: &str) {}
    fn say(&mut self, text: String) {
        self.said.push(text);
    }
    fn app_name(&self) -> &str {
        "Notebook"
    }
}

/// Lets the page answer, then hands the answers to the pages, as a frame would.
async fn frame(app: &mut Notebook, ctx: &egui::Context) {
    wait(20).await;
    cloud::pump(app, ctx);
}

#[wasm_bindgen_test]
async fn the_open_and_save_pages_work_through_the_page() {
    install();
    let ctx = egui::Context::default();
    let mut app = Notebook {
        cloud: CloudFiles::new(),
        share: ShareDialog::new(),
        presence: PresenceBar::new(),
        opened: Vec::new(),
        saved: Vec::new(),
        said: Vec::new(),
    };
    app.cloud.connect(PageStorage::new(|| ()));
    cloud::start(&mut app);
    frame(&mut app, &ctx).await;
    assert_eq!(app.cloud.account.as_ref().map(|a| a.display.as_str()), Some("Ada"));
    // Into a folder by its path, as the Open page does, and back.
    let plans = app.cloud.mine.entries.iter().find(|e| e.kind == EntryKind::Folder).cloned().expect("the root lists a folder");
    cloud::browse(&mut app, &plans.path);
    frame(&mut app, &ctx).await;
    assert_eq!(app.cloud.mine.path, "home/Plans");
    assert!(log().contains(r#"["list","home/Plans"]"#), "{}", log());
    cloud::browse(&mut app, "");
    frame(&mut app, &ctx).await;
    let entry = app.cloud.mine.entries.iter().find(|e| e.id == "home/a.txt").cloned().expect("the root lists a.txt");

    cloud::open_entry(&mut app, entry, None);
    frame(&mut app, &ctx).await;
    assert_eq!(app.opened, [("a.txt".to_owned(), vec![1, 2, 3])]);

    cloud::save(&mut app).unwrap();
    frame(&mut app, &ctx).await;
    assert_eq!(app.saved, [1]);
    assert_eq!(app.cloud.origins[&1].entry.etag.as_deref(), Some("2"));

    // Somebody else saves in the meantime: the next save is refused and never forced.
    bump("home/a.txt");
    cloud::save(&mut app).unwrap();
    frame(&mut app, &ctx).await;
    assert_eq!(app.saved, [1]);
    assert!(app.cloud.conflict.is_some(), "{:?}", app.said);
    assert!(log().contains(r#"["write","home/a.txt",[7,7],"2"]"#), "{}", log());
}
