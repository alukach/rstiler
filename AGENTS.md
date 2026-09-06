# AGENTS.md

## The parity tables move with the code

`README.md` carries two tables under **Parity with titiler** — *Endpoints* and
*Tile parameters* — plus a *Next, in order of value* list. They are this
project's map of what is and isn't done, and the reason anyone can tell a
prototype from a toy.

**Any change to what the server supports updates those tables in the same
commit.** A table that says `no` next to something that shipped is worse than no
table at all.

- New route → add a row to *Endpoints*, and fill the titiler column honestly
  (check <https://developmentseed.org/titiler/endpoints/cog/> rather than
  guessing).
- New query parameter → add a row to *Tile parameters*, using titiler's name and
  semantics wherever one exists. Don't invent a spelling titiler already has.
- Widening support → move the cell from `no` to `partial` to `yes`, and say in
  the note what is still missing. "partial" with no explanation is a dead cell.
- Shipping a roadmap item → delete it from *Next, in order of value*.
- Removing support → the row stays, the cell goes back to `no`.

The same honesty applies to **What has been verified**. Those are claims about
measured behaviour: the `gdalwarp` byte-identity, the `proj4rs` agreement, the
five-of-eight example count. If a change could move any of them, re-measure and
edit the number — never leave a stale figure standing because it reads better.

The long-form version of the comparison is published at
<https://claude.ai/code/artifact/c51b79f7-e768-4024-b2e2-68c08b72ddfe>. It is a
snapshot, not a living doc — the README is the source of truth. Re-publish it
only when asked.

## Verify before claiming

```
./fixtures/setup.sh --all          # once; GDAL + ~7 MB of corpus
python3 fixtures/serve.py &        # :8099
wrangler dev &                     # :8787
python3 fixtures/check.py
rustc --test src/tiling.rs   -o /tmp/t && /tmp/t
rustc --test src/colormap.rs -o /tmp/c && /tmp/c
```

`check.py` is the gate for anything touching the tile pipeline, and its
`KNOWN_GAPS` dict is the gap list in executable form. Three rules:

- A fixture that fails **without** a `KNOWN_GAPS` entry is a regression. Fix it,
  don't add an entry to silence it.
- A fixture that starts passing prints `FIXED`. Delete its entry, and move the
  matching row out of the README's gap table in the same change.
- When you add a feature, add its assertion to `check.py`. That is what stops
  the parity table from outrunning what actually works.

Timing claims come from the `Server-Timing` header on a real remote COG, not
from a fixture on localhost.

## Conventions

- `// ponytail:` marks a deliberate shortcut. It names the ceiling and the
  upgrade path. Don't remove one without doing the upgrade it describes.
- One job per module — see the Layout table in the README. Code that doesn't fit
  an existing file's job probably wants a new one, not a wider file.
- `src/tiling.rs` stays dependency-free so it can be test-compiled on its own
  without a wasm toolchain. Keep it that way.
