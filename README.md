# craft-page-storage

A web page's own file storage, as a place the Craft apps can open from and save to.

The Craft apps share one storage contract, `craft-cloud`, and one set of Open, Save and Places pages for it (`craft_ui::cloud`). A storage place is something the user picks: git, WebDAV, a server folder, CollabCraft, and so on. Each one is a backend in its own crate or repo, and the apps don't grow a client for each one ([tangency-labs/solidcraft#7420](https://github.com/tangency-labs/solidcraft/issues/7420)).

This crate is that backend for **the page that serves the app**. A page that shows a Craft app's web build, such as a cloud drive, a team portal, or a page that embeds the app with `@craft/app`, puts its storage on `window.craftStorage`. The app's own Open and Save pages then list, open and save files there. The page never hands the app a password or a token: the app only calls the page's functions.

## In an app

The app already uses `craft_ui::cloud` (the shared Open and Save pages). Its web shell connects this backend once the page offers storage, the same way it connects any other backend. It checks on each frame until then, because a page may hand the storage over after the app has started. `@craft/app` ([storytold/craft-libs#413](https://github.com/storytold/craft-libs/pull/413)) sets `window.craftStorage` on the frame as soon as the frame holds the app's page. That is usually before the app starts, but not always, so the app should keep looking.

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
craft-page-storage = { git = "https://github.com/hayone1/craft-page-storage", rev = "…" }
```

```rust
// In the app's update, until the page has offered its storage.
if !app.cloud.available() && craft_page_storage::available() {
    let ctx = ctx.clone();
    app.cloud.connect(craft_page_storage::PageStorage::new(move || ctx.request_repaint()));
}
```

`available()` only reads a few properties, so checking it on each frame is cheap. Without `window.craftStorage` it stays `false` and nothing changes. On a native build it is always `false`.

## In a page

`window.craftStorage` is an object with these functions. Each one can return its answer or a promise of it.

| Function | Answer |
|---|---|
| `account()` | `{ display, capabilities }`: who is signed in. Reject with code `unauthenticated` when nobody is. |
| `list(path)` | The files and folders in one folder, as entries. `""` is the top folder, and a folder's `path` is what opens it. |
| `read(id)` | The file's bytes: a `Uint8Array`, an `ArrayBuffer` or a `Blob`. |
| `write(id, bytes, ifMatch)` | The stored file, as an entry. `ifMatch` is the etag the file was read at, or `undefined`. Reject with code `conflict` when it is no longer the stored version, so a save never overwrites someone else's work. |
| `create(folder, name, bytes)` | The new file, as an entry. |
| `sharedWithMe()` | Optional. The entries other people shared with this user. |
| `signIn()` | Optional. Settles once the user has signed in, or gave up. |
| `signOut()` | Optional. |

An **entry** is `{ id, name, path, kind, size, modified, mime, etag, writable, room, sharedBy }`. Only `id` is required. It is the page's own handle for the file, and the app only hands it back. `path` and `room` default to `id`, `name` to the last part of `id`, `kind` to `"file"` (the other kind is `"folder"`) and `writable` to `true`. Paths use `/`, because the Open page finds a folder's parent by cutting at the last `/`. These are the fields of `craft_cloud::Entry`.

A **rejection** can carry a `code`: `conflict`, `unauthenticated`, `not_linked` or `unreachable`. Anything else is a refusal, and the app shows its `message`.

**Capabilities** use craft-cloud's names. This backend offers files, `etags` when the page lists it, and files shared with the user when the page has `sharedWithMe`. It leaves out the rest (sharing, contacts, conversations), because other backends handle those and a button for them would fail here.

The page owns timeouts. A call that never settles leaves its transfer showing as in progress. The bytes may come from the app's own frame, so check them with `ArrayBuffer.isView`, not `instanceof`.

A small example, with the files kept in memory:

```js
const files = new Map(); // id -> { bytes, etag }
const entry = (id) => ({ id, size: files.get(id).bytes.length, etag: files.get(id).etag });
const conflict = () => Object.assign(new Error("Someone else saved this file."), { code: "conflict" });

window.craftStorage = {
  account: () => ({ display: "Ada", capabilities: ["etags"] }),
  list: (path) => [...files.keys()].filter((id) => id.startsWith(path)).map(entry),
  read: (id) => files.get(id).bytes,
  write(id, bytes, ifMatch) {
    const etag = files.get(id)?.etag;
    if (ifMatch !== undefined && ifMatch !== etag) throw conflict();
    files.set(id, { bytes: bytes.slice(), etag: String(Number(etag ?? 0) + 1) });
    return entry(id);
  },
  create(folder, name, bytes) {
    const id = folder ? `${folder}/${name}` : name;
    files.set(id, { bytes: bytes.slice(), etag: "1" });
    return entry(id);
  },
};
```

The page can set `window.craftStorage` before or after the app starts. Once the app has connected it, the object must stay in place: the app looks it up again for every call.

## Status

`craft-cloud` and `craft_ui::cloud` are proposed in [storytold/craft-libs#160](https://github.com/storytold/craft-libs/pull/160). This crate pins that pull request's commit, so an app that uses it needs the same craft-libs revision. Once #160 lands, the pin moves to `storytold/craft-libs`.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
# The browser half runs in Node with the WASM test runner:
cargo clippy --all-targets --locked --target wasm32-unknown-unknown -- -D warnings
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test --locked --target wasm32-unknown-unknown
```

The WASM tests drive the real Open and Save pages from `craft_ui::cloud` against a fake page: listing, opening, saving with an etag, and a save that is refused because someone else saved first.

## License

MIT or Apache-2.0, at your choice, like craft-libs.
