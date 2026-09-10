# AGENTS.md

## Drop-in compatibility with titiler is the goal

The README states it: anything this server answers, it answers the way titiler
would. That is a constraint on every change, not an aspiration.

- **Never silently differ.** A parameter we do not implement is refused with a
  400. Adding one that is accepted and ignored is the single change that must
  never land — a client cannot tell a tile was rendered without it. The README
  explains why, and `conformance.py` asserts it.
- **Copy titiler's spelling.** Take the parameter name, its accepted forms and
  its status codes from `titiler/src/titiler/core/tests/test_factories.py`, not
  from memory. Accepting *more* than titiler is fine — `bidx=1,2,3` alongside
  `bidx=1&bidx=2&bidx=3` — accepting something *different* is not.
- When you implement one, delete it from `UNIMPLEMENTED` in `src/lib.rs`, from
  `UNIMPLEMENTED` in `fixtures/conformance.py`, and from the README's gap
  table. All three, in the same change.

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

## Write the failing test first

**Every fix starts with a test that fails for the reason you are about to fix.**
Not after. The order is the point: a test written after the fix proves the code
runs, while a test written before it proves the test can detect the bug at all.
A check that would have passed against the broken code is worth nothing, and
you cannot tell which kind you have written unless you watched it fail.

The loop, every time:

1. Reproduce the failure as a check — a fixture in `fixtures/`, a case in
   `check.py` or `conformance.py`, or a `#[test]` in a dependency-free module.
2. Run it. **Watch it fail, and read the failure.** If it passes, or fails for
   a different reason than the bug, the check is wrong — fix the check before
   touching the code.
3. Make the change.
4. Run it again and watch it pass, then run the whole gate.

This is not ceremony, and this repo has the scars to prove it:

- The trailing-overview COG fix shipped with a fixture built *afterwards*. The
  first version of that fixture put the overview IFDs 117 KB in, inside a
  single read — it would have passed against the unfixed code. It only became
  a real test once it was enlarged, which was luck, not method.
- The tile cache once served pixels from a previous build, and a gate that had
  not been made to fail first would have graded the wrong binary as green.

When a bug cannot be reproduced locally — it needs a 1.4 GB file, or a
Cloudflare CPU limit — say so in the commit message, and add the smallest
fixture that exercises the same code path. `layout_trailing_overviews.tif` is
that: `gdaladdo` reproduces NLCD's layout in 4 MB.

## Verify before claiming

```
./fixtures/setup.sh --all          # once; GDAL + ~7 MB of corpus
python3 fixtures/serve.py &        # :8099
wrangler dev &                     # :8787
python3 fixtures/check.py                  # formats and rendering
python3 fixtures/conformance.py            # titiler's contract
./fixtures/unit.sh                         # tiling, colormap, query
cargo fmt --check
cargo clippy --target wasm32-unknown-unknown --release --all-features -- -D warnings
```

The fixture harness is linted too:

```
uvx ruff check fixtures/
shellcheck fixtures/*.sh
```

CI runs exactly these, so a green local run means a green pipeline. Clippy is
`-D warnings` and both linters are clean — fix a lint rather than allowing it,
and when a lint is genuinely wrong, `#[allow]` or `# noqa:` it with a comment
saying why. There are three such suppressions today and each names its reason.

Do not add `ruff format` or drop the `rustfmt::skip` on `BUILTIN`: the aligned
tables in `check.py` and `colormap.rs` are aligned on purpose.

`check.py` is the gate for anything touching the tile pipeline, and its
`KNOWN_GAPS` dict is the gap list in executable form. Three rules:

- A fixture that fails **without** a `KNOWN_GAPS` entry is a regression. Fix it,
  don't add an entry to silence it.
- A fixture that starts passing prints `FIXED`. Delete its entry, and move the
  matching row out of the README's gap table in the same change.
- When you add a feature, add its assertion to `check.py`. That is what stops
  the parity table from outrunning what actually works.
- A fixture that renders a blank tile fails. If it is *legitimately* empty, put
  it in `ALL_NODATA` with the evidence — not in `KNOWN_GAPS`.

`conformance.py` is the other gate: it encodes titiler's contract, and every
assertion cites the titiler test it came from. When you add a parameter or an
endpoint, copy titiler's spelling and its status codes rather than inventing
your own, and add the matching check. Its `UNIMPLEMENTED` dict shrinks by
deleting entries — never by softening an assertion so it passes.

Timing claims come from the `Server-Timing` header on a real remote COG, not
from a fixture on localhost.

## Conventions

- `// ponytail:` marks a deliberate shortcut. It names the ceiling and the
  upgrade path. Don't remove one without doing the upgrade it describes.
- One job per module — see the Layout table in the README. Code that doesn't fit
  an existing file's job probably wants a new one, not a wider file.
- `src/tiling.rs` stays dependency-free so it can be test-compiled on its own
  without a wasm toolchain. Keep it that way.
