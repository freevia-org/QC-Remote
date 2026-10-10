A guide and tutorial for the `quadcortex` crate, written for a Rust developer
who wants to talk to a Quad Cortex, or to analyse its USB traffic, without
having read the codebase first. You are expected to know Rust. You are not
expected to know the device protocol; the guide introduces it as it is needed.

> **Independent and unofficial.** `quadcortex` is part of QC Remote, an
> independent project. It is not affiliated with, authorized, sponsored,
> endorsed, or supported by Neural DSP Technologies Oy. Neural DSP and Quad
> Cortex are trademarks of Neural DSP Technologies Oy and are named only to
> identify compatibility.

## How the examples are labelled

Every Rust snippet in this guide is a documentation test, so `cargo test -p
quadcortex --doc` compiles it. There are exactly two kinds:

| Fence | What it means |
| --- | --- |
| `rust` | Compiled **and run** by the documentation tests. It needs no hardware: it feeds the library synthetic bytes that the snippet builds itself, or this crate's own sanitized fixture. |
| `rust,no_run` | Compiled but **never executed**. It is the shape of code that would sit next to a real USB handle. Each one is labelled *compiles but was not run against a device*. |

> **What was and was not verified.** The authors of this guide did not run any
> snippet against a Quad Cortex. The offline snippets prove what the library
> does with the bytes they construct. They do not prove that a unit sends those
> bytes, in that order, with those fields. Where the guide says something about
> device behaviour, it says it because a doc comment or a test in the crate
> says it, and it points at that item. Where nothing establishes a point, the
> guide says *not established here*.

## Contents

1. [What the library is and is not](crate::guide::overview): scope, status,
   stability, minimum Rust version.
2. [Concepts and the layer map](crate::guide::concepts): reports, frames,
   messages, request ids, and which module owns which job.
3. [Quick start, offline](crate::guide::offline_quick_start): frame a command,
   reassemble reports, decode a captured trace, decode state.
4. [Tutorial: read the current preset and watch state updates](crate::guide::read_state)
5. [Tutorial: sending commands safely](crate::guide::send_commands): ranges,
   expected-state guards, confirmations, request ids, persistent writes.
6. [Sessions, handshake, suspension and recovery](crate::guide::sessions)
7. [Backups](crate::guide::backups)
8. [Transports: what the crate provides and what the host provides](crate::guide::transports)
9. [Testing your code without a device](crate::guide::testing)
10. [Troubleshooting](crate::guide::troubleshooting)
11. [FAQ and limits](crate::guide::limits)

## Runnable examples in the repository

Besides the snippets, the crate ships offline examples under `examples/`:

```text
cargo run -p quadcortex --example frame_round_trip
cargo run -p quadcortex --example replay_trace
cargo run -p quadcortex --example state_decoder
cargo run -p quadcortex --example command_tour
cargo run -p quadcortex --example state_watcher
cargo run -p quadcortex --example startup_walkthrough
```

The first three are described in the crate README. `command_tour`,
`state_watcher` and `startup_walkthrough` accompany this guide and are
referenced from sections 5, 4 and 6.

## Where this guide lives

The pages are plain Markdown files under `docs/guide/` in the crate. They are
compiled into the API documentation by a module that exists only while rustdoc
runs, so it adds nothing to the compiled library or to its public surface. Build
the documentation locally with:

```text
cargo doc -p quadcortex --no-deps --open
```

The pages use rustdoc's intra-doc links. Read in a plain Markdown viewer, those
links show as unlinked text, and the hidden lines that set up some snippets
(lines starting with `#`) are visible.
