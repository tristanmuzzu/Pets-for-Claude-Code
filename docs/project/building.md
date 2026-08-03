# Building and releasing

[← README](../../README.md)

## Build

Needs Node 20+, a Rust toolchain, and on Windows the MSVC C++ build tools.

```bash
npm install
npm run sprites
npm run app:build
```

`npm run app:dev` runs it with hot reload.

`npm run dev` serves the frontend alone in a browser, with a demo loop standing
in for real sessions. `?panel=welcome` and `?panel=doctor` render those panels
so they can be worked on without a build or a broken machine to point them at.

## Tests

```bash
npm test                                   # the pure display logic and the pet manifests
cargo test --manifest-path src-tauri/Cargo.toml
```

The JavaScript tests run under Node's own runner — no framework, no config
file. `src/derive.js` holds every judgement about whether a card is telling the
truth, deliberately separated from rendering so it can be tested without a
window, and every function takes `now` so the awkward moments can be examined
rather than waited for.

What is *not* tested: transparent windows, the tray, the hit testing, and
anything that needs a real Claude Code session. Those are checked by hand
against the release checklist below.

## Releasing

1. Bump the version in `package.json`, `src-tauri/Cargo.toml` and
   `src-tauri/tauri.conf.json`. Write `docs/releases/vX.Y.Z.md`.
2. `npm run verify:release` — asserts all three agree, that the notes exist, and
   that the tag matches.
3. `npm test && cargo test --manifest-path src-tauri/Cargo.toml`
4. Push the tag. CI runs the same checks before it builds anything, then
   produces a **draft** release.
5. Work through the checklist, then publish the draft.

### Release checklist

Anything not verified on real hardware is recorded as such in the notes rather
than assumed.

**Every platform**

- [ ] Fresh install: the welcome panel appears once, installs the hooks, and
      does not come back after Done, the close button, or a restart.
- [ ] Upgrade from the previous version: the pet is where it was left, and the
      existing `config.json` is neither reset nor rewritten.
- [ ] A session start, a tool call, a permission prompt, and a completion each
      reach the card within about a third of a second.
- [ ] Killing Claude Code mid-turn retires the card within ~10s rather than
      leaving it "running".
- [ ] Setup check: all green, and the connection test reports the session you
      ran during it.
- [ ] Clicks pass through the transparent area to whatever is underneath.
- [ ] Uninstall removes the hooks from `~/.claude/settings.json`.

**Windows**

- [ ] No console window flashes on any menu action or hook.
- [ ] Tray blink on completion, and clicking the tray stops it.
- [ ] Start with Windows survives a reboot.
- [ ] Dragging to a second monitor, then unplugging it, leaves the pet reachable.

**macOS / Linux**

- [ ] Builds at all. If there is no hardware to check on, say so in the notes.
