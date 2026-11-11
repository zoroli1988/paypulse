# 第三方组件与许可

这个文件由 `app/tools/license-scan/scan.py` 生成，**不要手改**。

统计口径：`cargo tree --target x86_64-pc-windows-gnu --edges normal,build`，即真正链接进 `paypulse-app.exe` 的全部依赖。
共 **227** 个第三方 crate。

## 一眼看结论

**没有任何 GPL 系依赖。** 全部为宽松许可，可以随 AGPL-3.0 的外壳一起分发。

| 许可 | 包数 | 分发时的义务 |
|---|---:|---|
<!-- 双许可（如 `MIT OR Apache-2.0`）在每个分支各计一次，所以合计大于包数 -->
| MIT | 197 | 保留版权与许可声明。 |
| Apache-2.0 | 155 | 保留版权与许可声明。 |
| Unicode-3.0 | 19 | ICU 系（icu_* / zerovec / yoke …），要求随附其许可原文。 |
| Unlicense | 7 | 公有领域捐献，无义务。 |
| BSD-3-Clause | 6 | 要求保留版权声明，且不得用作者名义为衍生品背书。 |
| Zlib | 6 | 要求不得声称本软件由原作者编写。 |
| MPL-2.0 | 5 | 文件级 copyleft。不改这些文件就只需声明；改了就得公开改动。 |
| 0BSD | 1 | 保留版权与许可声明。 |
| CC0-1.0 | 1 | 公有领域捐献，无义务。 |
| MIT-0 | 1 | 保留版权与许可声明。 |

### 需要单独留意的几种

**MPL-2.0** 是唯一带 copyleft 的：`cssparser`、`cssparser-macros`、`dtoa-short`、`option-ext`、`selectors`。它按**文件**生效——这几个 crate 一行没改，所以义务只有一条：告诉拿到二进制的人它们是 MPL-2.0，并且能拿到源码。下表每行的 crate 名都能点，链到 crates.io 的对应版本，那是最直接的源码获取途径。

**Unicode-3.0** 来自 ICU4X 那一串（渲染中文界面时的 Unicode 属性查询要用）。它要求随附许可原文，原文在上表指定的 crate 包内 `LICENSE` 文件里。

## 完整清单

| crate | 版本 | 许可 |
|---|---|---|
| [`adler2`](https://crates.io/crates/adler2/2.0.1) | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| [`aho-corasick`](https://crates.io/crates/aho-corasick/1.1.5) | 1.1.5 | Unlicense OR MIT |
| [`alloc-no-stdlib`](https://crates.io/crates/alloc-no-stdlib/3.0.0) | 3.0.0 | BSD-3-Clause |
| [`alloc-stdlib`](https://crates.io/crates/alloc-stdlib/0.3.0) | 0.3.0 | BSD-3-Clause |
| [`anyhow`](https://crates.io/crates/anyhow/1.0.104) | 1.0.104 | MIT OR Apache-2.0 |
| [`autocfg`](https://crates.io/crates/autocfg/1.5.1) | 1.5.1 | Apache-2.0 OR MIT |
| [`base64`](https://crates.io/crates/base64/0.23.1) | 0.23.1 | MIT OR Apache-2.0 |
| [`bit-set`](https://crates.io/crates/bit-set/0.8.0) | 0.8.0 | Apache-2.0 OR MIT |
| [`bit-vec`](https://crates.io/crates/bit-vec/0.8.0) | 0.8.0 | Apache-2.0 OR MIT |
| [`bitflags`](https://crates.io/crates/bitflags/2.13.2) | 2.13.2 | MIT OR Apache-2.0 |
| [`block-buffer`](https://crates.io/crates/block-buffer/0.10.4) | 0.10.4 | MIT OR Apache-2.0 |
| [`brotli`](https://crates.io/crates/brotli/9.1.1) | 9.1.1 | BSD-3-Clause AND MIT |
| [`brotli-decompressor`](https://crates.io/crates/brotli-decompressor/6.3.1) | 6.3.1 | BSD-3-Clause/MIT |
| [`bytemuck`](https://crates.io/crates/bytemuck/1.25.2) | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| [`byteorder`](https://crates.io/crates/byteorder/1.5.0) | 1.5.0 | Unlicense OR MIT |
| [`byteorder-lite`](https://crates.io/crates/byteorder-lite/0.1.0) | 0.1.0 | Unlicense OR MIT |
| [`bytes`](https://crates.io/crates/bytes/1.12.1) | 1.12.1 | MIT |
| [`camino`](https://crates.io/crates/camino/1.2.6) | 1.2.6 | MIT OR Apache-2.0 |
| [`cargo-platform`](https://crates.io/crates/cargo-platform/0.1.9) | 0.1.9 | MIT OR Apache-2.0 |
| [`cargo_metadata`](https://crates.io/crates/cargo_metadata/0.19.2) | 0.19.2 | MIT |
| [`cargo_toml`](https://crates.io/crates/cargo_toml/1.0.1) | 1.0.1 | Apache-2.0 OR MIT |
| [`cc`](https://crates.io/crates/cc/1.11.1) | 1.11.1 | MIT OR Apache-2.0 |
| [`cfb`](https://crates.io/crates/cfb/0.14.0) | 0.14.0 | MIT |
| [`cfg-if`](https://crates.io/crates/cfg-if/1.0.5) | 1.0.5 | MIT OR Apache-2.0 |
| [`chrono`](https://crates.io/crates/chrono/0.4.45) | 0.4.45 | MIT OR Apache-2.0 |
| [`cookie`](https://crates.io/crates/cookie/0.18.2) | 0.18.2 | MIT OR Apache-2.0 |
| [`cpufeatures`](https://crates.io/crates/cpufeatures/0.2.17) | 0.2.17 | MIT OR Apache-2.0 |
| [`crc32fast`](https://crates.io/crates/crc32fast/1.5.5) | 1.5.5 | MIT OR Apache-2.0 |
| [`crossbeam-channel`](https://crates.io/crates/crossbeam-channel/0.5.17) | 0.5.17 | MIT OR Apache-2.0 |
| [`crossbeam-utils`](https://crates.io/crates/crossbeam-utils/0.8.23) | 0.8.23 | MIT OR Apache-2.0 |
| [`crypto-common`](https://crates.io/crates/crypto-common/0.1.7) | 0.1.7 | MIT OR Apache-2.0 |
| [`cssparser`](https://crates.io/crates/cssparser/0.37.0) | 0.37.0 | MPL-2.0 |
| [`cssparser-macros`](https://crates.io/crates/cssparser-macros/0.7.1) | 0.7.1 | MPL-2.0 |
| [`ctor`](https://crates.io/crates/ctor/1.0.13) | 1.0.13 | Apache-2.0 OR MIT |
| [`darling`](https://crates.io/crates/darling/0.24.1) | 0.24.1 | MIT |
| [`darling_core`](https://crates.io/crates/darling_core/0.24.1) | 0.24.1 | MIT |
| [`darling_macro`](https://crates.io/crates/darling_macro/0.24.1) | 0.24.1 | MIT |
| [`deranged`](https://crates.io/crates/deranged/0.5.9) | 0.5.9 | MIT OR Apache-2.0 |
| [`derive_more`](https://crates.io/crates/derive_more/2.1.1) | 2.1.1 | MIT |
| [`derive_more-impl`](https://crates.io/crates/derive_more-impl/2.1.1) | 2.1.1 | MIT |
| [`digest`](https://crates.io/crates/digest/0.10.7) | 0.10.7 | MIT OR Apache-2.0 |
| [`dirs`](https://crates.io/crates/dirs/7.0.0) | 7.0.0 | MIT OR Apache-2.0 |
| [`dirs-sys`](https://crates.io/crates/dirs-sys/0.5.0) | 0.5.0 | MIT OR Apache-2.0 |
| [`displaydoc`](https://crates.io/crates/displaydoc/0.2.7) | 0.2.7 | MIT OR Apache-2.0 |
| [`dom_query`](https://crates.io/crates/dom_query/0.28.0) | 0.28.0 | MIT |
| [`dpi`](https://crates.io/crates/dpi/0.1.2) | 0.1.2 | Apache-2.0 AND MIT |
| [`dtoa`](https://crates.io/crates/dtoa/1.0.11) | 1.0.11 | MIT OR Apache-2.0 |
| [`dtoa-short`](https://crates.io/crates/dtoa-short/0.3.5) | 0.3.5 | MPL-2.0 |
| [`dunce`](https://crates.io/crates/dunce/1.0.5) | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| [`dyn-clone`](https://crates.io/crates/dyn-clone/1.0.20) | 1.0.20 | MIT OR Apache-2.0 |
| [`embed-resource`](https://crates.io/crates/embed-resource/3.0.12) | 3.0.12 | MIT |
| [`equivalent`](https://crates.io/crates/equivalent/1.0.3) | 1.0.3 | Apache-2.0 OR MIT |
| [`erased-serde`](https://crates.io/crates/erased-serde/0.4.11) | 0.4.11 | MIT OR Apache-2.0 |
| [`fastrand`](https://crates.io/crates/fastrand/2.5.0) | 2.5.0 | Apache-2.0 OR MIT |
| [`fdeflate`](https://crates.io/crates/fdeflate/0.3.7) | 0.3.7 | MIT OR Apache-2.0 |
| [`find-msvc-tools`](https://crates.io/crates/find-msvc-tools/0.1.15) | 0.1.15 | MIT OR Apache-2.0 |
| [`flate2`](https://crates.io/crates/flate2/1.1.11) | 1.1.11 | MIT OR Apache-2.0 |
| [`fnv`](https://crates.io/crates/fnv/1.0.7) | 1.0.7 | Apache-2.0 / MIT |
| [`foldhash`](https://crates.io/crates/foldhash/0.2.0) | 0.2.0 | Zlib |
| [`form_urlencoded`](https://crates.io/crates/form_urlencoded/1.2.2) | 1.2.2 | MIT OR Apache-2.0 |
| [`generic-array`](https://crates.io/crates/generic-array/0.14.7) | 0.14.7 | MIT |
| [`getrandom`](https://crates.io/crates/getrandom/0.3.4) | 0.3.4 | MIT OR Apache-2.0 |
| [`getrandom`](https://crates.io/crates/getrandom/0.4.3) | 0.4.3 | MIT OR Apache-2.0 |
| [`glob`](https://crates.io/crates/glob/0.3.4) | 0.3.4 | MIT OR Apache-2.0 |
| [`hashbrown`](https://crates.io/crates/hashbrown/0.17.1) | 0.17.1 | MIT OR Apache-2.0 |
| [`hashbrown`](https://crates.io/crates/hashbrown/0.12.3) | 0.12.3 | MIT OR Apache-2.0 |
| [`heck`](https://crates.io/crates/heck/0.5.0) | 0.5.0 | MIT OR Apache-2.0 |
| [`html5ever`](https://crates.io/crates/html5ever/0.39.0) | 0.39.0 | MIT OR Apache-2.0 |
| [`http`](https://crates.io/crates/http/1.5.0) | 1.5.0 | MIT OR Apache-2.0 |
| [`ico`](https://crates.io/crates/ico/0.6.0) | 0.6.0 | MIT |
| [`icu_collections`](https://crates.io/crates/icu_collections/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_locale_core`](https://crates.io/crates/icu_locale_core/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_normalizer`](https://crates.io/crates/icu_normalizer/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_normalizer_data`](https://crates.io/crates/icu_normalizer_data/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_properties`](https://crates.io/crates/icu_properties/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_properties_data`](https://crates.io/crates/icu_properties_data/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`icu_provider`](https://crates.io/crates/icu_provider/2.4.0) | 2.4.0 | Unicode-3.0 |
| [`ident_case`](https://crates.io/crates/ident_case/1.0.1) | 1.0.1 | MIT/Apache-2.0 |
| [`idna`](https://crates.io/crates/idna/1.1.0) | 1.1.0 | MIT OR Apache-2.0 |
| [`idna_adapter`](https://crates.io/crates/idna_adapter/1.2.2) | 1.2.2 | Apache-2.0 OR MIT |
| [`image`](https://crates.io/crates/image/0.25.10) | 0.25.10 | MIT OR Apache-2.0 |
| [`indexmap`](https://crates.io/crates/indexmap/2.14.2) | 2.14.2 | Apache-2.0 OR MIT |
| [`indexmap`](https://crates.io/crates/indexmap/1.9.3) | 1.9.3 | Apache-2.0 OR MIT |
| [`infer`](https://crates.io/crates/infer/0.22.0) | 0.22.0 | MIT |
| [`itoa`](https://crates.io/crates/itoa/1.0.19) | 1.0.19 | MIT OR Apache-2.0 |
| [`json-patch`](https://crates.io/crates/json-patch/4.2.0) | 4.2.0 | MIT/Apache-2.0 |
| [`jsonptr`](https://crates.io/crates/jsonptr/0.7.1) | 0.7.1 | MIT OR Apache-2.0 |
| [`keyboard-types`](https://crates.io/crates/keyboard-types/0.8.3) | 0.8.3 | MIT OR Apache-2.0 |
| [`libc`](https://crates.io/crates/libc/0.2.191) | 0.2.191 | MIT OR Apache-2.0 |
| [`litemap`](https://crates.io/crates/litemap/0.8.3) | 0.8.3 | Unicode-3.0 |
| [`lock_api`](https://crates.io/crates/lock_api/0.4.14) | 0.4.14 | MIT OR Apache-2.0 |
| [`log`](https://crates.io/crates/log/0.4.35) | 0.4.35 | MIT OR Apache-2.0 |
| [`markup5ever`](https://crates.io/crates/markup5ever/0.39.0) | 0.39.0 | MIT OR Apache-2.0 |
| [`memchr`](https://crates.io/crates/memchr/2.8.3) | 2.8.3 | Unlicense OR MIT |
| [`mime`](https://crates.io/crates/mime/0.3.18) | 0.3.18 | MIT OR Apache-2.0 |
| [`miniz_oxide`](https://crates.io/crates/miniz_oxide/0.9.1) | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| [`miniz_oxide`](https://crates.io/crates/miniz_oxide/0.8.9) | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| [`moxcms`](https://crates.io/crates/moxcms/0.8.1) | 0.8.1 | BSD-3-Clause OR Apache-2.0 |
| [`muda`](https://crates.io/crates/muda/0.20.0) | 0.20.0 | Apache-2.0 OR MIT |
| [`new_debug_unreachable`](https://crates.io/crates/new_debug_unreachable/1.0.6) | 1.0.6 | MIT |
| [`num-conv`](https://crates.io/crates/num-conv/0.2.2) | 0.2.2 | MIT OR Apache-2.0 |
| [`num-traits`](https://crates.io/crates/num-traits/0.2.19) | 0.2.19 | MIT OR Apache-2.0 |
| [`once_cell`](https://crates.io/crates/once_cell/1.21.4) | 1.21.4 | MIT OR Apache-2.0 |
| [`option-ext`](https://crates.io/crates/option-ext/0.2.0) | 0.2.0 | MPL-2.0 |
| [`parking_lot`](https://crates.io/crates/parking_lot/0.12.5) | 0.12.5 | MIT OR Apache-2.0 |
| [`parking_lot_core`](https://crates.io/crates/parking_lot_core/0.9.12) | 0.9.12 | MIT OR Apache-2.0 |
| [`percent-encoding`](https://crates.io/crates/percent-encoding/2.3.2) | 2.3.2 | MIT OR Apache-2.0 |
| [`phf`](https://crates.io/crates/phf/0.13.1) | 0.13.1 | MIT |
| [`phf_codegen`](https://crates.io/crates/phf_codegen/0.13.1) | 0.13.1 | MIT |
| [`phf_generator`](https://crates.io/crates/phf_generator/0.13.1) | 0.13.1 | MIT |
| [`phf_macros`](https://crates.io/crates/phf_macros/0.13.1) | 0.13.1 | MIT |
| [`phf_shared`](https://crates.io/crates/phf_shared/0.13.1) | 0.13.1 | MIT |
| [`pin-project-lite`](https://crates.io/crates/pin-project-lite/0.2.17) | 0.2.17 | Apache-2.0 OR MIT |
| [`plist`](https://crates.io/crates/plist/1.10.3) | 1.10.3 | MIT |
| [`png`](https://crates.io/crates/png/0.18.1) | 0.18.1 | MIT OR Apache-2.0 |
| [`potential_utf`](https://crates.io/crates/potential_utf/0.1.6) | 0.1.6 | Unicode-3.0 |
| [`powerfmt`](https://crates.io/crates/powerfmt/0.2.1) | 0.2.1 | MIT OR Apache-2.0 |
| [`precomputed-hash`](https://crates.io/crates/precomputed-hash/0.1.1) | 0.1.1 | MIT |
| [`proc-macro2`](https://crates.io/crates/proc-macro2/1.0.107) | 1.0.107 | MIT OR Apache-2.0 |
| [`pxfm`](https://crates.io/crates/pxfm/0.1.30) | 0.1.30 | BSD-3-Clause OR Apache-2.0 |
| [`quick-xml`](https://crates.io/crates/quick-xml/0.42.0) | 0.42.0 | MIT |
| [`quote`](https://crates.io/crates/quote/1.0.47) | 1.0.47 | MIT OR Apache-2.0 |
| [`raw-window-handle`](https://crates.io/crates/raw-window-handle/0.6.2) | 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| [`regex`](https://crates.io/crates/regex/1.13.1) | 1.13.1 | MIT OR Apache-2.0 |
| [`regex-automata`](https://crates.io/crates/regex-automata/0.4.18) | 0.4.18 | MIT OR Apache-2.0 |
| [`regex-syntax`](https://crates.io/crates/regex-syntax/0.8.11) | 0.8.11 | MIT OR Apache-2.0 |
| [`rustc-hash`](https://crates.io/crates/rustc-hash/2.1.3) | 2.1.3 | Apache-2.0 OR MIT |
| [`rustc_version`](https://crates.io/crates/rustc_version/0.4.1) | 0.4.1 | MIT OR Apache-2.0 |
| [`same-file`](https://crates.io/crates/same-file/1.0.6) | 1.0.6 | Unlicense/MIT |
| [`schemars`](https://crates.io/crates/schemars/0.8.22) | 0.8.22 | MIT |
| [`schemars_derive`](https://crates.io/crates/schemars_derive/0.8.22) | 0.8.22 | MIT |
| [`scopeguard`](https://crates.io/crates/scopeguard/1.2.0) | 1.2.0 | MIT OR Apache-2.0 |
| [`selectors`](https://crates.io/crates/selectors/0.38.0) | 0.38.0 | MPL-2.0 |
| [`semver`](https://crates.io/crates/semver/1.0.28) | 1.0.28 | MIT OR Apache-2.0 |
| [`serde`](https://crates.io/crates/serde/1.0.229) | 1.0.229 | MIT OR Apache-2.0 |
| [`serde-untagged`](https://crates.io/crates/serde-untagged/0.1.9) | 0.1.9 | MIT OR Apache-2.0 |
| [`serde_core`](https://crates.io/crates/serde_core/1.0.229) | 1.0.229 | MIT OR Apache-2.0 |
| [`serde_derive`](https://crates.io/crates/serde_derive/1.0.229) | 1.0.229 | MIT OR Apache-2.0 |
| [`serde_derive_internals`](https://crates.io/crates/serde_derive_internals/0.29.1) | 0.29.1 | MIT OR Apache-2.0 |
| [`serde_json`](https://crates.io/crates/serde_json/1.0.154) | 1.0.154 | MIT OR Apache-2.0 |
| [`serde_repr`](https://crates.io/crates/serde_repr/0.1.21) | 0.1.21 | MIT OR Apache-2.0 |
| [`serde_spanned`](https://crates.io/crates/serde_spanned/1.1.3) | 1.1.3 | MIT OR Apache-2.0 |
| [`serde_with`](https://crates.io/crates/serde_with/3.24.0) | 3.24.0 | MIT OR Apache-2.0 |
| [`serde_with_macros`](https://crates.io/crates/serde_with_macros/3.24.0) | 3.24.0 | MIT OR Apache-2.0 |
| [`serialize-to-javascript`](https://crates.io/crates/serialize-to-javascript/0.1.2) | 0.1.2 | MIT OR Apache-2.0 |
| [`serialize-to-javascript-impl`](https://crates.io/crates/serialize-to-javascript-impl/0.1.2) | 0.1.2 | MIT OR Apache-2.0 |
| [`servo_arc`](https://crates.io/crates/servo_arc/0.4.3) | 0.4.3 | MIT OR Apache-2.0 |
| [`sha2`](https://crates.io/crates/sha2/0.10.9) | 0.10.9 | MIT OR Apache-2.0 |
| [`shlex`](https://crates.io/crates/shlex/2.0.1) | 2.0.1 | MIT OR Apache-2.0 |
| [`simd-adler32`](https://crates.io/crates/simd-adler32/0.3.10) | 0.3.10 | MIT |
| [`siphasher`](https://crates.io/crates/siphasher/1.0.4) | 1.0.4 | MIT OR Apache-2.0 |
| [`smallvec`](https://crates.io/crates/smallvec/1.16.5) | 1.16.5 | MIT OR Apache-2.0 |
| [`softbuffer`](https://crates.io/crates/softbuffer/0.4.8) | 0.4.8 | MIT OR Apache-2.0 |
| [`stable_deref_trait`](https://crates.io/crates/stable_deref_trait/1.2.1) | 1.2.1 | MIT OR Apache-2.0 |
| [`static_vcruntime`](https://crates.io/crates/static_vcruntime/3.0.0) | 3.0.0 | MIT OR Apache-2.0 OR Zlib |
| [`string_cache`](https://crates.io/crates/string_cache/0.9.0) | 0.9.0 | MIT OR Apache-2.0 |
| [`string_cache_codegen`](https://crates.io/crates/string_cache_codegen/0.6.1) | 0.6.1 | MIT OR Apache-2.0 |
| [`strsim`](https://crates.io/crates/strsim/0.11.1) | 0.11.1 | MIT |
| [`syn`](https://crates.io/crates/syn/3.0.10) | 3.0.10 | MIT OR Apache-2.0 |
| [`syn`](https://crates.io/crates/syn/2.0.119) | 2.0.119 | MIT OR Apache-2.0 |
| [`synstructure`](https://crates.io/crates/synstructure/0.14.0) | 0.14.0 | MIT |
| [`tao`](https://crates.io/crates/tao/0.37.2) | 0.37.2 | Apache-2.0 |
| [`tauri`](https://crates.io/crates/tauri/2.12.5) | 2.12.5 | Apache-2.0 OR MIT |
| [`tauri-build`](https://crates.io/crates/tauri-build/2.7.4) | 2.7.4 | Apache-2.0 OR MIT |
| [`tauri-codegen`](https://crates.io/crates/tauri-codegen/2.7.4) | 2.7.4 | Apache-2.0 OR MIT |
| [`tauri-macros`](https://crates.io/crates/tauri-macros/2.7.4) | 2.7.4 | Apache-2.0 OR MIT |
| [`tauri-runtime`](https://crates.io/crates/tauri-runtime/2.12.2) | 2.12.2 | Apache-2.0 OR MIT |
| [`tauri-runtime-wry`](https://crates.io/crates/tauri-runtime-wry/2.12.3) | 2.12.3 | Apache-2.0 OR MIT |
| [`tauri-utils`](https://crates.io/crates/tauri-utils/2.10.2) | 2.10.2 | Apache-2.0 OR MIT |
| [`tauri-winres`](https://crates.io/crates/tauri-winres/0.3.7) | 0.3.7 | MIT |
| [`tendril`](https://crates.io/crates/tendril/0.5.1) | 0.5.1 | MIT OR Apache-2.0 |
| [`thiserror`](https://crates.io/crates/thiserror/2.0.21) | 2.0.21 | MIT OR Apache-2.0 |
| [`thiserror-impl`](https://crates.io/crates/thiserror-impl/2.0.21) | 2.0.21 | MIT OR Apache-2.0 |
| [`time`](https://crates.io/crates/time/0.3.55) | 0.3.55 | MIT OR Apache-2.0 |
| [`time-core`](https://crates.io/crates/time-core/0.1.9) | 0.1.9 | MIT OR Apache-2.0 |
| [`time-macros`](https://crates.io/crates/time-macros/0.2.32) | 0.2.32 | MIT OR Apache-2.0 |
| [`tinystr`](https://crates.io/crates/tinystr/0.8.5) | 0.8.5 | Unicode-3.0 |
| [`tokio`](https://crates.io/crates/tokio/1.54.1) | 1.54.1 | MIT |
| [`toml`](https://crates.io/crates/toml/1.2.1+spec-1.1.0) | 1.2.1+spec-1.1.0 | MIT OR Apache-2.0 |
| [`toml_datetime`](https://crates.io/crates/toml_datetime/1.1.3+spec-1.1.0) | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| [`toml_parser`](https://crates.io/crates/toml_parser/1.1.5+spec-1.1.0) | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 |
| [`toml_writer`](https://crates.io/crates/toml_writer/1.2.0+spec-1.1.0) | 1.2.0+spec-1.1.0 | MIT OR Apache-2.0 |
| [`tracing`](https://crates.io/crates/tracing/0.1.44) | 0.1.44 | MIT |
| [`tracing-core`](https://crates.io/crates/tracing-core/0.1.36) | 0.1.36 | MIT |
| [`tray-icon`](https://crates.io/crates/tray-icon/0.25.1) | 0.25.1 | MIT OR Apache-2.0 |
| [`typeid`](https://crates.io/crates/typeid/1.0.3) | 1.0.3 | MIT OR Apache-2.0 |
| [`typenum`](https://crates.io/crates/typenum/1.20.1) | 1.20.1 | MIT OR Apache-2.0 |
| [`unicode-ident`](https://crates.io/crates/unicode-ident/1.0.27) | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| [`unicode-segmentation`](https://crates.io/crates/unicode-segmentation/1.13.3) | 1.13.3 | MIT OR Apache-2.0 |
| [`url`](https://crates.io/crates/url/2.5.8) | 2.5.8 | MIT OR Apache-2.0 |
| [`urlpattern`](https://crates.io/crates/urlpattern/0.6.0) | 0.6.0 | MIT |
| [`utf8_iter`](https://crates.io/crates/utf8_iter/1.1.0) | 1.1.0 | Apache-2.0 OR MIT |
| [`utf_types`](https://crates.io/crates/utf_types/1.0.0) | 1.0.0 | Apache-2.0 OR MIT |
| [`uuid`](https://crates.io/crates/uuid/1.29.2) | 1.29.2 | Apache-2.0 OR MIT |
| [`version_check`](https://crates.io/crates/version_check/0.9.5) | 0.9.5 | MIT/Apache-2.0 |
| [`walkdir`](https://crates.io/crates/walkdir/2.5.0) | 2.5.0 | Unlicense/MIT |
| [`web-time`](https://crates.io/crates/web-time/1.1.0) | 1.1.0 | MIT OR Apache-2.0 |
| [`web_atoms`](https://crates.io/crates/web_atoms/0.2.6) | 0.2.6 | MIT OR Apache-2.0 |
| [`webview2-com`](https://crates.io/crates/webview2-com/0.39.1) | 0.39.1 | MIT |
| [`webview2-com-macros`](https://crates.io/crates/webview2-com-macros/0.8.1) | 0.8.1 | MIT |
| [`webview2-com-sys`](https://crates.io/crates/webview2-com-sys/0.39.1) | 0.39.1 | MIT |
| [`winapi-util`](https://crates.io/crates/winapi-util/0.1.11) | 0.1.11 | Unlicense OR MIT |
| [`window-vibrancy`](https://crates.io/crates/window-vibrancy/0.8.1) | 0.8.1 | Apache-2.0 OR MIT |
| [`windows`](https://crates.io/crates/windows/0.62.2) | 0.62.2 | MIT OR Apache-2.0 |
| [`windows-collections`](https://crates.io/crates/windows-collections/0.3.2) | 0.3.2 | MIT OR Apache-2.0 |
| [`windows-core`](https://crates.io/crates/windows-core/0.62.2) | 0.62.2 | MIT OR Apache-2.0 |
| [`windows-future`](https://crates.io/crates/windows-future/0.3.2) | 0.3.2 | MIT OR Apache-2.0 |
| [`windows-implement`](https://crates.io/crates/windows-implement/0.60.2) | 0.60.2 | MIT OR Apache-2.0 |
| [`windows-interface`](https://crates.io/crates/windows-interface/0.59.3) | 0.59.3 | MIT OR Apache-2.0 |
| [`windows-link`](https://crates.io/crates/windows-link/0.2.1) | 0.2.1 | MIT OR Apache-2.0 |
| [`windows-numerics`](https://crates.io/crates/windows-numerics/0.3.1) | 0.3.1 | MIT OR Apache-2.0 |
| [`windows-result`](https://crates.io/crates/windows-result/0.4.1) | 0.4.1 | MIT OR Apache-2.0 |
| [`windows-strings`](https://crates.io/crates/windows-strings/0.5.1) | 0.5.1 | MIT OR Apache-2.0 |
| [`windows-sys`](https://crates.io/crates/windows-sys/0.61.2) | 0.61.2 | MIT OR Apache-2.0 |
| [`windows-threading`](https://crates.io/crates/windows-threading/0.2.1) | 0.2.1 | MIT OR Apache-2.0 |
| [`windows-version`](https://crates.io/crates/windows-version/0.1.7) | 0.1.7 | MIT OR Apache-2.0 |
| [`winnow`](https://crates.io/crates/winnow/1.0.5) | 1.0.5 | MIT |
| [`writeable`](https://crates.io/crates/writeable/0.6.4) | 0.6.4 | Unicode-3.0 |
| [`wry`](https://crates.io/crates/wry/0.57.2) | 0.57.2 | Apache-2.0 OR MIT |
| [`yoke`](https://crates.io/crates/yoke/0.8.4) | 0.8.4 | Unicode-3.0 |
| [`yoke-derive`](https://crates.io/crates/yoke-derive/0.8.4) | 0.8.4 | Unicode-3.0 |
| [`zerofrom`](https://crates.io/crates/zerofrom/0.1.8) | 0.1.8 | Unicode-3.0 |
| [`zerofrom-derive`](https://crates.io/crates/zerofrom-derive/0.1.8) | 0.1.8 | Unicode-3.0 |
| [`zerotrie`](https://crates.io/crates/zerotrie/0.2.5) | 0.2.5 | Unicode-3.0 |
| [`zerovec`](https://crates.io/crates/zerovec/0.11.9) | 0.11.9 | Unicode-3.0 |
| [`zerovec-derive`](https://crates.io/crates/zerovec-derive/0.11.7) | 0.11.7 | Unicode-3.0 |
| [`zmij`](https://crates.io/crates/zmij/1.0.23) | 1.0.23 | MIT |

## 本项目自己的 crate

- `paypulse-app` 0.1.0
- `paypulse-clock` 0.1.0
- `paypulse-core` 0.1.0

其中 `paypulse-core` 是私有仓（计薪引擎），不随本仓的 AGPL 授权。
