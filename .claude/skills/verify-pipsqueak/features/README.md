# Feature map

What a user can do with Pipsqueak, and what proves each one works. One file per
feature. A drive that covers only `hint` is incomplete while the rest sit here
unproven; the map exists so that gap is visible rather than assumed away.

| Feature | File | Driven by `drive.mjs` |
| --- | --- | --- |
| Hover hint | [hint.md](hint.md) | yes, `drive.mjs hint` |
| Cards (the stack of turn cards) | not written yet | no |
| Right-click menu | not written yet | no |
| Drag to move the pet | not written yet | no |
| Click to hide the cards | not written yet | no |

Adding one means a file here and a function in `drive.mjs` with its own named
checks, so a failure says which behaviour broke rather than that something did.

## Where the surfaces are

- `#pet`, `#hint`, `#chips`, `#menu`, `#panel` in `index.html`.
- Interaction wiring in `src/main.js`, `wireInteraction()`.
- Cards render from `#card-template` in `index.html`.

## What the unit tests already cover

`test/` covers APNG decoding, derivation, look rows and pet data. Anything you
can only see while the app is running is not in there, which is the whole reason
this skill exists.
