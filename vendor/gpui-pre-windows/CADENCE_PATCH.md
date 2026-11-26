# Cadence Windows patch

Upstream: gpui-pre-windows 0.3.9, from crates.io; license: LICENSE-APACHE.

Two source changes (ported unchanged in behaviour from 0.3.7):

1. `src/clipboard.rs`: copying an image also offers the classic bitmap format
   (`CF_DIB`, composited over white) next to PNG, because many apps (Paint,
   Photos, Office, browsers, chat apps) only paste that.
2. `src/directx_renderer.rs`: debug shaders are embedded with `include_str!`, the
   alpha-correction include is inlined, and `D3DCompile` reads memory instead of
   `D3DCompileFromFile` reading the build machine's Cargo registry. Optimized
   builds continue using upstream's precompiled shaders without changes.

Keep both when updating GPUI until upstream carries them.
