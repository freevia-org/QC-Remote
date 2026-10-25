# Third-party notices

QC Remote is built from third-party open-source components. This file lists
every third-party package locked by the repository's four lockfiles, grouped by
license family, in two parts:

- **Shipped components** are compiled into, or bundled with, a distributed
  build: the Windows app, the device broker, or the Android app. They are the
  crates reachable over normal and build dependencies from
  `apps/windows/src-tauri` and `services/device-broker` (for
  `x86_64-pc-windows-msvc`) and `packages/rust/quadcortex-android` (for
  `aarch64-linux-android`), the npm packages reachable over runtime
  dependencies from the `apps/windows` and `apps/android` workspaces, and the
  Android app's locked Maven runtime classpath.
- **Development tooling (not distributed)** is everything else the lockfiles
  lock: compilers, bundlers, test runners, image tooling, and the packages only
  the hosted Cloudflare relay uses. It builds, tests or deploys QC Remote but is
  not distributed with it.

The four lockfiles are the root Rust workspace's `Cargo.lock`,
`apps/windows/src-tauri/Cargo.lock` (the Windows Tauri app is deliberately
excluded from the root workspace, see `exclude` in `Cargo.toml`, and resolves
its own lockfile), the npm workspace's `package-lock.json`, and
`apps/android/android/app/gradle.lockfile`. Regenerate this file with
`node scripts/verify-third-party-notices.mjs --write`, which reads all four;
without `--write` the same script fails when this file, the inventory, the
license texts or the source offer drift from the lockfiles.

Each dependency retains its own license and, where applicable, its own
copyright notices; nothing here modifies those terms. The complete license
and notice texts shipped with each build are collected in
[`legal/THIRD_PARTY-LICENSE-TEXTS.txt`](legal/THIRD_PARTY-LICENSE-TEXTS.txt),
and [`legal/THIRD_PARTY-LICENSE-INVENTORY.json`](legal/THIRD_PARTY-LICENSE-INVENTORY.json)
records every shipped component's exact version, license, upstream source, and
the SHA-256 of each of its texts. Exact-version source archives for the shipped
copyleft components are listed in
[`legal/THIRD_PARTY-SOURCE-OFFER.md`](legal/THIRD_PARTY-SOURCE-OFFER.md); it
covers every shipped component under MPL-2.0 or a GPL-family license, and
development tooling, which is not distributed, carries no source offer.

Two bundled, non-package-manager components have their own notice files
alongside their content in this repository:

- IBM Plex Sans (SIL Open Font License 1.1) —
  `packages/typescript/qc-theme/assets/fonts/LICENSE.txt`
- The `pyquadcortex` community protocol notice (MIT), covering the vendored
  protobuf schema in `packages/rust/quadcortex/proto/` —
  `packages/rust/quadcortex/licenses/pyquadcortex-MIT.txt` (the same text
  every build ships as `legal/COMMUNITY-PROTOCOL-LICENSE.txt`)

This file is regenerated from the lockfiles; it is not exhaustive legal
advice. License headings are normalized SPDX expressions: `/` is written `OR`,
the operands of a plain choice are sorted, and the license names Android
libraries' Maven POMs use are mapped to their SPDX identifiers. The inventory
keeps each component's license exactly as its own manifest or POM declares it.

## Shipped components

These components are compiled into, or bundled with, the Windows app, the
device broker, or the Android app. The machine-readable inventory records each
one's exact version, and the source offer covers every one of them under a
copyleft license.

### Rust crates

#### (Apache-2.0 OR MIT) AND BSD-3-Clause (1)

encoding_rs

#### (Apache-2.0 OR MIT) AND Unicode-3.0 (1)

unicode-ident

#### 0BSD OR Apache-2.0 OR MIT (1)

adler2

#### Apache-2.0 (8)

prost, prost-build, prost-derive, prost-types, rmcp, rmcp-macros, sync_wrapper, tao

#### Apache-2.0 AND ISC (1)

ring

#### Apache-2.0 AND MIT (1)

dpi

#### Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT (2)

linux-raw-sys, rustix

#### Apache-2.0 OR BSD-2-Clause OR MIT (1)

zerocopy

#### Apache-2.0 OR BSD-3-Clause (2)

moxcms, pxfm

#### Apache-2.0 OR BSL-1.0 (1)

ryu

#### Apache-2.0 OR CC0-1.0 OR MIT-0 (1)

dunce

#### Apache-2.0 OR ISC OR MIT (3)

hyper-rustls, rustls, rustls-native-certs

#### Apache-2.0 OR MIT (277)

aes, anyhow, arc-swap, async-broadcast, async-channel, async-executor, async-fs, async-io, async-lock, async-net, async-process, async-recursion, async-signal, async-task, async-trait, atomic-waker, autocfg, base64, basic-udev, bit-set, bit-vec, bitflags, block-buffer, blocking, bs58, camino, cargo_toml, cargo-platform, cc, cesu8, cfg-expr, cfg-if, chacha20, chrono, cipher, cmake, concurrent-queue, cookie, core_detect, core-foundation, core-foundation-sys, core-graphics, core-graphics-types, cpubits, cpufeatures, crc32fast, crossbeam-channel, crossbeam-queue, crossbeam-utils, crypto-common, ctor, ctr, dbus, dbus-secret-service, defmt, defmt-macros, defmt-parser, deranged, digest, dirs, dirs-sys, displaydoc, downcast-rs, dtoa, dyn-clone, either, embed_plist, enumflags2, enumflags2_derive, equivalent, erased-serde, errno, event-listener, event-listener-strategy, fastrand, fdeflate, field-offset, find-msvc-tools, fixedbitset, flate2, fnv, foreign-types, foreign-types-macros, foreign-types-shared, form_urlencoded, futures, futures-channel, futures-core, futures-executor, futures-io, futures-lite, futures-macro, futures-sink, futures-task, futures-util, getrandom, glob, hashbrown, heck, hex, html5ever, http, httparse, httpdate, hybrid-array, iana-time-zone, ident_case, idna, idna_adapter, image, indexmap, inout, ipnet, itertools, itoa, jni, jni-macros, jni-sys, jni-sys-macros, jobserver, json-patch, jsonptr, keyboard-types, keyring, libappindicator, libappindicator-sys, libc, libdbus-sys, lock_api, log, markup5ever, mime, muda, multimap, multiversion, multiversion_no_op, multiversion-macros, num-conv, num-traits, once_cell, openssl-probe, ordered-stream, parking, parking_lot, parking_lot_core, pastey, percent-encoding, petgraph, pin-project-lite, piper, pkg-config, png, polling, pollster, powerfmt, ppv-lite86, prettyplease, proc-macro-crate, proc-macro-error, proc-macro-error-attr, proc-macro2, quinn, quinn-proto, quinn-udp, quote, rand, rand_chacha, rand_core, rand_pcg, ref-cast, ref-cast-impl, regex, regex-automata, regex-syntax, reqwest, rustc_version, rustc-hash, rustls-pki-types, rustls-platform-verifier, rustls-platform-verifier-android, rustversion, scoped-tls, scopeguard, security-framework, security-framework-sys, semver, serde, serde_core, serde_derive, serde_derive_internals, serde_json, serde_path_to_error, serde_repr, serde_spanned, serde_urlencoded, serde_with, serde_with_macros, serde-untagged, serialize-to-javascript, serialize-to-javascript-impl, servo_arc, sha1, sha2, shlex, signal-hook, signal-hook-registry, simd_cesu8, simdutf8, siphasher, smallvec, socket2, softbuffer, sse-stream, stable_deref_trait, string_cache, string_cache_codegen, swift-rs, syn, system-configuration, system-configuration-sys, system-deps, task-local, tauri, tauri-build, tauri-codegen, tauri-macros, tauri-plugin-single-instance, tauri-runtime, tauri-runtime-wry, tauri-utils, tempfile, tendril, thiserror, thiserror-impl, time, time-core, time-macros, tokio-rustls, toml, toml_datetime, toml_edit, toml_parser, toml_writer, tray-icon, tungstenite, typed-path, typeid, typenum, unicode-segmentation, url, utf-8, utf8_iter, uuid, version_check, web_atoms, web-time, webbrowser, window-vibrancy, windows, windows_x86_64_msvc, windows-collections, windows-core, windows-future, windows-implement, windows-interface, windows-link, windows-numerics, windows-registry, windows-result, windows-strings, windows-sys, windows-targets, windows-threading, windows-version, wry, zeroize, zeroize_derive

#### Apache-2.0 OR MIT OR Zlib (13)

bytemuck, dispatch2, lru-slab, miniz_oxide, objc2-app-kit, objc2-core-foundation, objc2-core-graphics, objc2-exception-helper, objc2-io-surface, objc2-web-kit, raw-window-handle, tinyvec, tinyvec_macros

#### Apache-2.0 WITH LLVM-exception (1)

target-lexicon

#### BSD-3-Clause (1)

subtle

#### CDLA-Permissive-2.0 (1)

webpki-roots

#### ISC (3)

libloading, rustls-webpki, untrusted

#### ISC AND (Apache-2.0 OR ISC) (1)

aws-lc-rs

#### ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) (1)

aws-lc-sys

#### MIT (128)

ashpd, atk, atk-sys, axum, axum-core, block2, bytes, cairo-rs, cairo-sys-rs, cargo_metadata, cfb, cfg_aliases, combine, darling, darling_core, darling_macro, data-encoding, derive_more, derive_more-impl, dlib, dlopen2, dlopen2_derive, dom_query, embed-resource, endi, fs_extra, gdk, gdk-pixbuf, gdk-pixbuf-sys, gdk-sys, gdkwayland-sys, generic-array, gio, gio-sys, glib, glib-macros, glib-sys, gobject-sys, gtk, gtk-sys, gtk3-macros, h2, hidapi, http-body, http-body-util, hyper, hyper-util, ico, infer, javascriptcore-rs, javascriptcore-rs-sys, memoffset, mio, new_debug_unreachable, nix, objc2, objc2-encode, objc2-foundation, pango, pango-sys, phf, phf_codegen, phf_generator, phf_macros, phf_shared, plist, precomputed-hash, protoc-bin-vendored, protoc-bin-vendored-linux-aarch_64, protoc-bin-vendored-linux-ppcle_64, protoc-bin-vendored-linux-s390_64, protoc-bin-vendored-linux-x86_32, protoc-bin-vendored-linux-x86_64, protoc-bin-vendored-macos-aarch_64, protoc-bin-vendored-macos-x86_64, protoc-bin-vendored-win32, quick-xml, rfd, schannel, schemars, schemars_derive, simd-adler32, slab, soup3, soup3-sys, strsim, synstructure, tauri-winres, tokio, tokio-macros, tokio-stream, tokio-tungstenite, tokio-util, tower, tower-http, tower-layer, tower-service, tracing, tracing-attributes, tracing-core, try-lock, urlencoding, urlpattern, version-compare, vswhom, vswhom-sys, want, wayland-backend, wayland-client, wayland-protocols, wayland-scanner, wayland-sys, webkit2gtk, webkit2gtk-sys, webview2-com, webview2-com-macros, webview2-com-sys, winnow, winreg, zbus, zbus_macros, zbus_names, zcheapstr, zip, zmij, zvariant, zvariant_derive, zvariant_utils

#### MIT AND BSD-3-Clause (1)

matchit

#### MIT OR Unlicense (13)

aho-corasick, byteorder, byteorder-lite, csv, csv-core, jiff, jiff-core, jiff-tzdb, jiff-tzdb-platform, memchr, same-file, walkdir, winapi-util

#### MPL-2.0 (5)

cssparser, cssparser-macros, dtoa-short, option-ext, selectors

#### Unicode-3.0 (18)

icu_collections, icu_locale_core, icu_normalizer, icu_normalizer_data, icu_properties, icu_properties_data, icu_provider, litemap, potential_utf, tinystr, writeable, yoke, yoke-derive, zerofrom, zerofrom-derive, zerotrie, zerovec, zerovec-derive

#### Unlicense (1)

ksni

#### Zlib (2)

foldhash, zlib-rs

### npm packages

#### 0BSD (1)

tslib

#### Apache-2.0 OR MIT (1)

@tauri-apps/api

#### ISC (7)

cliui, get-caller-file, require-main-filename, set-blocking, which-module, y18n, yargs-parser

#### MIT (28)

@capacitor/android, @capacitor/core, @capacitor/status-bar, ansi-regex, ansi-styles, camelcase, color-convert, color-name, decamelize, dijkstrajs, emoji-regex, find-up, is-fullwidth-code-point, locate-path, p-limit, p-locate, p-try, path-exists, pngjs, qrcode, react, react-dom, require-directory, scheduler, string-width, strip-ansi, wrap-ansi, yargs

### Android libraries (Maven)

#### Apache-2.0 (57)

androidx.activity:activity, androidx.annotation:annotation, androidx.annotation:annotation-experimental, androidx.annotation:annotation-jvm, androidx.appcompat:appcompat, androidx.appcompat:appcompat-resources, androidx.arch.core:core-common, androidx.arch.core:core-runtime, androidx.collection:collection, androidx.collection:collection-jvm, androidx.concurrent:concurrent-futures, androidx.coordinatorlayout:coordinatorlayout, androidx.core:core, androidx.core:core-ktx, androidx.core:core-splashscreen, androidx.core:core-viewtree, androidx.cursoradapter:cursoradapter, androidx.customview:customview, androidx.drawerlayout:drawerlayout, androidx.emoji2:emoji2, androidx.emoji2:emoji2-views-helper, androidx.fragment:fragment, androidx.interpolator:interpolator, androidx.lifecycle:lifecycle-common, androidx.lifecycle:lifecycle-livedata, androidx.lifecycle:lifecycle-livedata-core, androidx.lifecycle:lifecycle-process, androidx.lifecycle:lifecycle-runtime, androidx.lifecycle:lifecycle-viewmodel, androidx.lifecycle:lifecycle-viewmodel-savedstate, androidx.loader:loader, androidx.profileinstaller:profileinstaller, androidx.resourceinspection:resourceinspection-annotation, androidx.savedstate:savedstate, androidx.startup:startup-runtime, androidx.tracing:tracing, androidx.vectordrawable:vectordrawable, androidx.vectordrawable:vectordrawable-animated, androidx.versionedparcelable:versionedparcelable, androidx.viewpager:viewpager, androidx.webkit:webkit, com.google.guava:listenablefuture, com.squareup.okhttp3:okhttp, com.squareup.okio:okio, com.squareup.okio:okio-jvm, org.apache.cordova:framework, org.jetbrains:annotations, org.jetbrains.kotlin:kotlin-stdlib, org.jetbrains.kotlin:kotlin-stdlib-common, org.jetbrains.kotlin:kotlin-stdlib-jdk7, org.jetbrains.kotlin:kotlin-stdlib-jdk8, org.jetbrains.kotlinx:kotlinx-coroutines-android, org.jetbrains.kotlinx:kotlinx-coroutines-bom, org.jetbrains.kotlinx:kotlinx-coroutines-core, org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm, org.jspecify:jspecify, org.rustls:rustls-platform-verifier

## Development tooling (not distributed)

These packages are locked in the same lockfiles but only build, test, lint or
deploy QC Remote: compilers and bundlers, test runners, image tooling, and the
packages only the hosted Cloudflare relay uses. None of them is distributed
with a QC Remote build, so they carry no source offer. They are listed so the
Rust/npm development dependency record is explicit. Android test and debug-only
configurations are outside this distributed-component inventory.

### Rust crates

#### Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT (3)

wasi, wasip2, wit-bindgen

#### Apache-2.0 OR BSD-2-Clause OR MIT (1)

zerocopy-derive

#### Apache-2.0 OR BSD-3-Clause OR MIT (2)

num_enum, num_enum_derive

#### Apache-2.0 OR LGPL-2.1-or-later OR MIT (1)

r-efi

#### Apache-2.0 OR MIT (34)

android_system_properties, bumpalo, hermit-abi, iana-time-zone-haiku, js-sys, ndk, ndk-context, ndk-sys, portable-atomic, portable-atomic-util, proptest, quick-error, rand_xorshift, rusty-fork, tao-macros, unarray, wait-timeout, wasm-bindgen, wasm-bindgen-futures, wasm-bindgen-macro, wasm-bindgen-macro-support, wasm-bindgen-shared, wasm-streams, web-sys, winapi, winapi-i686-pc-windows-gnu, winapi-x86_64-pc-windows-gnu, windows_aarch64_gnullvm, windows_aarch64_msvc, windows_i686_gnu, windows_i686_gnullvm, windows_i686_msvc, windows_x86_64_gnu, windows_x86_64_gnullvm

#### Apache-2.0 OR MIT OR Zlib (8)

objc2-cloud-kit, objc2-core-data, objc2-core-image, objc2-core-location, objc2-core-text, objc2-quartz-core, objc2-ui-kit, objc2-user-notifications

#### CDLA-Permissive-2.0 (1)

webpki-root-certs

#### MIT (4)

libredox, redox_syscall, redox_users, uds_windows

#### MIT OR Unlicense (1)

jiff-static

### npm packages

#### (CC0-1.0 OR MIT) (1)

type-fest

#### Apache-2.0 (47)

@cloudflare/workerd-darwin-64, @cloudflare/workerd-darwin-arm64, @cloudflare/workerd-linux-64, @cloudflare/workerd-linux-arm64, @cloudflare/workerd-windows-64, @img/sharp-darwin-arm64, @img/sharp-darwin-x64, @img/sharp-freebsd-wasm32, @img/sharp-linux-arm, @img/sharp-linux-arm64, @img/sharp-linux-ppc64, @img/sharp-linux-riscv64, @img/sharp-linux-s390x, @img/sharp-linux-x64, @img/sharp-linuxmusl-arm64, @img/sharp-linuxmusl-x64, @img/sharp-webcontainers-wasm32, @modelcontextprotocol/client, @modelcontextprotocol/core, @modelcontextprotocol/server, @typescript/typescript-aix-ppc64, @typescript/typescript-darwin-arm64, @typescript/typescript-darwin-x64, @typescript/typescript-freebsd-arm64, @typescript/typescript-freebsd-x64, @typescript/typescript-linux-arm, @typescript/typescript-linux-arm64, @typescript/typescript-linux-loong64, @typescript/typescript-linux-mips64el, @typescript/typescript-linux-ppc64, @typescript/typescript-linux-riscv64, @typescript/typescript-linux-s390x, @typescript/typescript-linux-x64, @typescript/typescript-netbsd-arm64, @typescript/typescript-netbsd-x64, @typescript/typescript-openbsd-arm64, @typescript/typescript-openbsd-x64, @typescript/typescript-sunos-x64, @typescript/typescript-win32-arm64, @typescript/typescript-win32-x64, baseline-browser-mapping, detect-libc, elementtree, expect-type, sharp, typescript, workerd

#### Apache-2.0 AND LGPL-3.0-or-later (3)

@img/sharp-win32-arm64, @img/sharp-win32-ia32, @img/sharp-win32-x64

#### Apache-2.0 AND LGPL-3.0-or-later AND MIT (1)

@img/sharp-wasm32

#### Apache-2.0 OR MIT (16)

@cloudflare/kv-asset-handler, @cloudflare/unenv-preset, @cloudflare/workers-types, @tauri-apps/cli, @tauri-apps/cli-darwin-arm64, @tauri-apps/cli-darwin-x64, @tauri-apps/cli-linux-arm-gnueabihf, @tauri-apps/cli-linux-arm64-gnu, @tauri-apps/cli-linux-arm64-musl, @tauri-apps/cli-linux-riscv64-gnu, @tauri-apps/cli-linux-x64-gnu, @tauri-apps/cli-linux-x64-musl, @tauri-apps/cli-win32-arm64-msvc, @tauri-apps/cli-win32-ia32-msvc, @tauri-apps/cli-win32-x64-msvc, wrangler

#### BlueOak-1.0.0 (10)

chownr, glob, lru-cache, minimatch, minipass, package-json-from-dist, path-scurry, rimraf, tar, yallist

#### BSD-2-Clause (1)

json-schema-typed

#### BSD-3-Clause (5)

fast-uri, js-base64, qs, source-map-js, tough-cookie

#### CC-BY-4.0 (1)

caniuse-lite

#### CC0-1.0 (1)

@speed-highlight/core

#### ISC (20)

@isaacs/fs-minipass, at-least-node, cli-width, electron-to-chromium, graceful-fs, inherits, ini, isexe, mute-stream, once, picocolors, sax, semver, setprototypeof, signal-exit, split2, which, wrappy, yaml, zod-to-json-schema

#### LGPL-3.0-or-later (10)

@img/sharp-libvips-darwin-arm64, @img/sharp-libvips-darwin-x64, @img/sharp-libvips-linux-arm, @img/sharp-libvips-linux-arm64, @img/sharp-libvips-linux-ppc64, @img/sharp-libvips-linux-riscv64, @img/sharp-libvips-linux-s390x, @img/sharp-libvips-linux-x64, @img/sharp-libvips-linuxmusl-arm64, @img/sharp-libvips-linuxmusl-x64

#### MIT (306)

@babel/code-frame, @babel/compat-data, @babel/core, @babel/generator, @babel/helper-annotate-as-pure, @babel/helper-compilation-targets, @babel/helper-create-class-features-plugin, @babel/helper-globals, @babel/helper-member-expression-to-functions, @babel/helper-optimise-call-expression, @babel/helper-plugin-utils, @babel/helper-replace-supers, @babel/helper-skip-transparent-expression-wrappers, @babel/helper-string-parser, @babel/helper-validator-identifier, @babel/helper-validator-option, @babel/helpers, @babel/parser, @babel/plugin-proposal-decorators, @babel/plugin-syntax-decorators, @babel/runtime, @babel/runtime-corejs3, @babel/template, @babel/traverse, @babel/types, @capacitor/app, @capacitor/browser, @capacitor/cli, @capacitor/ios, @cfworker/json-schema, @cloudflare/vitest-plugin, @cloudflare/workers-oauth-provider, @cspotcode/source-map-support, @emnapi/runtime, @esbuild/aix-ppc64, @esbuild/android-arm, @esbuild/android-arm64, @esbuild/android-x64, @esbuild/darwin-arm64, @esbuild/darwin-x64, @esbuild/freebsd-arm64, @esbuild/freebsd-x64, @esbuild/linux-arm, @esbuild/linux-arm64, @esbuild/linux-ia32, @esbuild/linux-loong64, @esbuild/linux-mips64el, @esbuild/linux-ppc64, @esbuild/linux-riscv64, @esbuild/linux-s390x, @esbuild/linux-x64, @esbuild/netbsd-arm64, @esbuild/netbsd-x64, @esbuild/openbsd-arm64, @esbuild/openbsd-x64, @esbuild/openharmony-arm64, @esbuild/sunos-x64, @esbuild/win32-arm64, @esbuild/win32-ia32, @esbuild/win32-x64, @hono/node-server, @img/colour, @inquirer/ansi, @inquirer/confirm, @inquirer/core, @inquirer/figures, @inquirer/type, @ionic/cli-framework-output, @ionic/utils-array, @ionic/utils-fs, @ionic/utils-object, @ionic/utils-process, @ionic/utils-stream, @ionic/utils-subprocess, @ionic/utils-terminal, @jridgewell/gen-mapping, @jridgewell/resolve-uri, @jridgewell/sourcemap-codec, @jridgewell/trace-mapping, @modelcontextprotocol/sdk, @msw/cloudflare, @msw/url, @mswjs/interceptors, @open-draft/until, @oxc-project/types, @poppinss/colors, @poppinss/dumper, @poppinss/exception, @rolldown/binding-android-arm-eabi, @rolldown/binding-android-arm64, @rolldown/binding-darwin-arm64, @rolldown/binding-darwin-x64, @rolldown/binding-freebsd-x64, @rolldown/binding-linux-arm-gnueabihf, @rolldown/binding-linux-arm64-gnu, @rolldown/binding-linux-arm64-musl, @rolldown/binding-linux-ppc64-gnu, @rolldown/binding-linux-s390x-gnu, @rolldown/binding-linux-x64-gnu, @rolldown/binding-linux-x64-musl, @rolldown/binding-openharmony-arm64, @rolldown/binding-win32-arm64-msvc, @rolldown/binding-win32-x64-msvc, @rolldown/plugin-babel, @rolldown/pluginutils, @sindresorhus/is, @types/chai, @types/debug, @types/deep-eql, @types/estree, @types/fs-extra, @types/gensync, @types/jsesc, @types/ms, @types/node, @types/qrcode, @types/react, @types/react-dom, @types/set-cookie-parser, @types/slice-ansi, @vitejs/plugin-react, @vitest/mocker, @vitest/spy, @xmldom/xmldom, accepts, agents, ajv, ajv-formats, assertion-error, astral-regex, balanced-match, base64-js, blake3-wasm, body-parser, bplist-parser, brace-expansion, browserslist, buffer-crc32, bytes, call-bind-apply-helpers, call-bound, capnweb, chai, cjs-module-lexer, commander, content-disposition, content-type, convert-source-map, cookie, cookie-signature, core-js-pure, cors, cron-schedule, cross-spawn, csstype, debug, define-lazy-prop, depd, dunder-proto, ee-first, empathic, encodeurl, env-paths, error-stack-parser-es, es-define-property, es-errors, es-module-lexer, es-object-atoms, esbuild, escalade, escape-html, estree-walker, etag, event-target-polyfill, eventsource, eventsource-parser, express, express-rate-limit, fast-deep-equal, fast-string-truncated-width, fast-string-width, fast-wrap-ansi, fd-slicer, fdir, finalhandler, flru, forwarded, fresh, fs-extra, fsevents, function-bind, gensync, get-east-asian-width, get-intrinsic, get-proto, gopd, has-symbols, hasown, headers-polyfill, hono, http-errors, iconv-lite, import-meta-resolve, ip-address, ipaddr.js, is-docker, is-node-process, is-promise, is-wsl, jose, js-tokens, jsesc, json-schema-traverse, json5, jsonfile, kleur, magic-string, math-intrinsics, media-typer, merge-descriptors, mime-db, mime-types, mimetext, miniflare, minizlib, ms, msw, nanoid, native-run, negotiator, node-releases, object-assign, object-inspect, obug, on-finished, open, outvariant, parseurl, partysocket, path-key, path-to-regexp, pathe, pend, picomatch, pkce-challenge, plist, postcss, prompts, proxy-addr, range-parser, raw-body, react-is, react-test-renderer, readable-stream, require-from-string, rettime, rolldown, router, safe-buffer, safer-buffer, send, serve-static, set-cookie-parser, shebang-command, shebang-regex, side-channel, side-channel-list, side-channel-map, side-channel-weakmap, sisteransi, slice-ansi, statuses, std-env, string_decoder, supports-color, tagged-tag, through2, tinybench, tinyexec, tinyglobby, tldts, tldts-core, toidentifier, tree-kill, type-is, undici, undici-types, unenv, universalify, unpipe, until-async, untildify, update-browserslist-db, util-deprecate, vary, verkit, vite, vitest, why-is-node-running, ws, xml2js, xmlbuilder, yauzl, youch, youch-core, zod

#### MIT-0 (1)

postal-mime

#### MPL-2.0 (12)

lightningcss, lightningcss-android-arm64, lightningcss-darwin-arm64, lightningcss-darwin-x64, lightningcss-freebsd-x64, lightningcss-linux-arm-gnueabihf, lightningcss-linux-arm64-gnu, lightningcss-linux-arm64-musl, lightningcss-linux-x64-gnu, lightningcss-linux-x64-musl, lightningcss-win32-arm64-msvc, lightningcss-win32-x64-msvc

#### Unlicense (1)

big-integer
