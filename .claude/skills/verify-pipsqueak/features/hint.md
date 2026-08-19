# Hover hint

## What it is

A one-line hint (`Click to hide the cards · right-click for options · drag to
move`) that appears beside the pet after the cursor has rested on it, and
disappears when the cursor leaves. It is the project's own element rather than a
`title` attribute, because the operating system shows a native tooltip after
half a second and half a second fires every time the cursor merely passes by.

`HINT_DELAY_MS` is 2500 (`src/main.js:76`). That delay is the feature, not an
implementation detail: it is what stops the hint firing at someone reaching past
the pet for something else.

## How to get to it (user POV)

Rest the cursor on the pet, bottom-right of the screen, and wait. Move away and
it goes.

## Driving it with drive.mjs

```bash
npm run dev
node .claude/skills/verify-pipsqueak/drive.mjs hint
```

`hintLifecycle()` dispatches real `PointerEvent`s at the pet's measured centre
and dwells with repeated `pointermove`, because a silent wait reads as a
departure (see the SKILL's gotcha section).

## What proves it works

Six checks, all on the element's own `hidden`:

| Moment | Expected | Guards against |
| --- | --- | --- |
| before the cursor arrives | hidden | showing unprompted |
| 1200ms into the hover | still hidden | firing at a passing cursor |
| 3000ms into the hover | showing | never appearing at all |
| while showing | positioned beside the pet, not at 0,0 | rendering off-screen |
| 400ms after leaving | hidden | `74f7262`, the hint lingering after the cursor left |
| second visit, 3000ms | showing again | `bb55af0` and `e242576`, one drag poisoning it so it never returned |

The last two are the regressions this feature keeps having. `pointerenter` and
`pointerleave` cannot be trusted on their own here: dragging the pet takes the
pointer out of the window under a grab, and the page is never told the cursor
left, so afterwards no crossing event arrives at all. The code reads arrival
from movement instead, which is why the drive has to move.

## Gotchas

- Viewport 0x0 parks the pet at -104,-104 and every hit test misses. Doctor
  catches it.
- The capture-phase `pointermove` listener on `document` (`src/main.js:1466`)
  cancels a pending hint on any move not over the pet.
- Under Tauri the same behaviour is additionally driven by the
  `pipsqueak://cursor` event (`src/main.js:1611`), which this browser-mode drive
  does not exercise. A hint bug that only reproduces in the real window belongs
  to that path, and needs `wayland-computer-use`.
