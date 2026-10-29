#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! A web page's own file storage, as a destination for the Craft apps' Open and Save pages.
//!
//! The Craft apps speak one storage contract, `craft-cloud`, and draw one Open, Save and Places
//! chrome for it ([`craft_ui::cloud`]). A destination is something the user picks: git, WebDAV,
//! a server folder, CollabCraft, or, with this crate, the page that serves the app. A page that
//! shows a Craft app's web build (a cloud drive, a team portal, a page that embeds it with
//! `@craft/app`) puts its storage on `window.craftStorage`. [`PageStorage`] is the
//! [`CloudBackend`](craft_ui::cloud::CloudBackend) that reaches it, so an app connects it like any
//! other backend and grows no client of its own:
//!
//! ```ignore
//! if craft_page_storage::available() {
//!     let ctx = ctx.clone();
//!     app.cloud.connect(craft_page_storage::PageStorage::new(move || ctx.request_repaint()));
//! }
//! ```
//!
//! Without `window.craftStorage`, and on a native build, [`available`] is `false` and the app
//! offers whatever it offered before.
//!
//! # What the page provides
//!
//! `window.craftStorage` is an object with these functions. Each may return its answer or a
//! promise of it.
//!
//! | Function | Answer |
//! |---|---|
//! | `account()` | `{ display, capabilities }`: who is signed in. Reject with code `unauthenticated` when nobody is. |
//! | `list(path)` | the files and folders in one folder, as entries. `""` is the root, and a folder's `path` is what opens it. |
//! | `read(id)` | the file's bytes: a `Uint8Array`, an `ArrayBuffer` or a `Blob`. |
//! | `write(id, bytes, ifMatch)` | the stored file, as an entry. Reject with code `conflict` when `ifMatch` (the etag the file was read at, or `undefined`) is no longer the stored version. |
//! | `create(folder, name, bytes)` | the new file, as an entry. |
//! | `sharedWithMe()` | optional: the entries other people shared with this user. |
//! | `signIn()` | optional: settles once the user has signed in (or gave up). |
//! | `signOut()` | optional. |
//!
//! An entry is `{ id, name, path, kind, size, modified, mime, etag, writable, room, sharedBy }`.
//! Only `id` is required: it is the page's own handle, and the app only hands it back. `path` and
//! `room` default to `id`, `name` to the last part of `id`, `kind` to `"file"` (the other value
//! is `"folder"`) and `writable` to `true`. Paths use `/`, because the pages find a folder's
//! parent by cutting at the last `/`. These are the fields of [`craft_cloud::Entry`].
//!
//! A rejection's `code` says what went wrong, as [`Fault`](craft_ui::cloud::Fault) does:
//! `conflict`, `unauthenticated`, `not_linked` or `unreachable`. Anything else is a refusal, and
//! the pages show its `message`.
//!
//! `capabilities` uses craft-cloud's names. This backend serves files, `etags` when the page lists
//! it, and what other people shared when the page has `sharedWithMe`. It drops the rest (sharing,
//! contacts, conversations): those belong to other backends, and a button for them would fail.
//!
//! The page owns timeouts: a call that never settles leaves its transfer showing as in progress.
//! `bytes` may come from the app's own frame, so a page that checks its type should use
//! `ArrayBuffer.isView`, not `instanceof`.

#[cfg(target_arch = "wasm32")]
mod page;
#[cfg(any(target_arch = "wasm32", test))]
mod parse;

#[cfg(target_arch = "wasm32")]
pub use page::{PageStorage, answer, available};

/// Whether the page offers its storage in this shape. Always `false` on a native build, which has
/// no page.
#[cfg(not(target_arch = "wasm32"))]
pub fn available() -> bool {
    false
}
