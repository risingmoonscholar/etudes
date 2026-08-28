# Finder platform spike — findings

Measured on macOS 26.2 / Xcode 26.4.1, against the gates in
`FINDER-SCRUB-PROTOCOL.md`. Every finding below was observed directly, most
of them twice: once when a wrong assumption failed silently, once when the
corrected experiment confirmed why.

## Transport verdict

A signed Action Extension hosted at `com.apple.ui-services` receives a Finder
selection as in-place originals: `loadInPlaceFileRepresentation` returns the
selected files' real paths with `inPlace=true`, on the user's own volume, for
every item selected. A bundled helper launched from the sandboxed extension
with an argv array receives those paths and runs. The protocol's central
unproven claim — that a selection can be bridged to a bundled engine without
broad entitlements, a daemon, or path copying — is now demonstrated for the
read side. Writing beside the selection (gates F3/F4) remains open.

## Gate results so far

- **F1 (identity and path bytes): pass; ordering unproven.** Every selected
  item arrived, none merged or dropped, and stored bytes were preserved to
  the extension — composed and decomposed names arrived distinct and intact,
  including names containing `$()`, backticks, quotes, semicolons, leading
  dashes, double spaces, and an embedded newline. Delivery order against
  selection order has not been tested and is not claimed.
- **F2 (no shell interpolation): pass.** A filename crafted to create a file
  if any layer interpolated it created nothing, searched for at every
  plausible working directory. No shell appears in the invocation path.
- **F7 (first datum): cancelling a hung request reaped the extension
  process.** No orphan remained. One datum, not a pass.

## Platform findings

1. **Unicode path bytes are rewritten at every process boundary.** Darwin's
   C-string bridging decomposes Unicode when a path crosses `exec` into a
   child's argv: a path recorded as composed (NFC) inside the extension
   arrived decomposed (NFD) in the helper. Separately, the legacy Services
   text pipeline delivers decomposed names while APFS stores what was
   written. Consequence for every tool in this suite: path bytes are not
   stable identifiers across process boundaries, and any comparison between
   a delivered path and a directory listing must be normalization-insensitive.
2. **The legacy Services route delivers file CONTENTS, not URLs.** A workflow
   or services-style delivery hands the extension `public.plain-text`
   representations — the platform reads every selected file to do it. Under
   this project's read-surface rule (Gate U), that disqualifies the route for
   any tool whose contract is names, sizes, and dates only. The Action
   Extension route with in-place URLs is the only delivery mode observed that
   does not open file contents.
3. **Hand-assembled extension bundles are refused silently.** A structurally
   valid `.appex` built outside Xcode's toolchain never registers — same
   result ad-hoc signed, Apple Development signed, and sandbox-entitled, with
   no log line, no crash, and no error anywhere. The toolchain bakes build
   metadata registration depends on. Extensions must be built as real Xcode
   targets.
4. **Xcode's own Action Extension template deadlocks against Finder on this
   macOS.** The template's headless configuration (`NSObject` request handler
   at `com.apple.services`) launches, parks at its XPC listener, and never
   receives the request; Finder shows an indeterminate progress dialog
   indefinitely. The configuration that executes is a view-controller
   principal hosted at `com.apple.ui-services` — the shape the system's own
   Markup extension uses.
5. **Activation rules are predicates; a wrong one is invisible.** A dict-form
   rule registers without error and matches nothing. No diagnostic exists;
   the action simply never appears.
6. **The principal class name carries the Swift module prefix.** A mismatch
   produces a launched host and a silent no-op, distinguishable from success
   only by instrumentation inside the extension.
7. **Build products self-register and shadow the installed copy.** Building
   an extension leaves the build directory's copy registered with the
   extension registry, silently superseding the installed one. Every test of
   an installed extension must first verify which copy the registry resolves.
8. **Failure is mute at every layer.** Six distinct misconfigurations across
   this spike produced zero diagnostics between them: no log, no crash
   report, no registry error. On this surface, an experiment that cannot
   observe its own execution proves nothing — instrument first, then test.

## Consequences already adopted

- Gate U carries the read-surface clause: the Finder path must exhibit the
  same file-content read surface as the CLI path, witnessed by tracing opens,
  because entitlements are capabilities and say nothing about behavior.
- Engine path comparisons must be normalization-insensitive (finding 1).
- The product's extensions are Xcode targets with plists correct at build
  time; post-build patching and registry state are verified, not assumed
  (findings 3, 7).

## Open

- F1 ordering; F3/F4 write-beside-the-selection; F5 File Provider locations;
  F6 result presentation; F8 lifecycle across enable/upgrade/removal; F9
  reveal-in-Finder. The write-beside experiment is next: one directory
  created beside the input under `user-selected.read-write`, or the refusal
  recorded.
