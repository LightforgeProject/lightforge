# Why this copy exists

iced_winit 0.14.1, unchanged but for one match arm, `window::Action::Resize` in
`src/lib.rs`, and `deprecated = "allow"` in `Cargo.toml`: a local crate gets no
`--cap-lints`, and CI builds with `-D warnings`.

On Wayland `request_inner_size` applies the size at once and returns it, and by
winit's contract no `Resized` follows. iced discarded that answer, so its layout,
hit testing and surface stayed at the old size while the compositor stretched the
buffer over the new one: clicks landed up to about 25 px off, until a manual
resize (issue #14, KDE Plasma with fractional scaling). egui fixed the same winit
behaviour the same way, emilk/egui#4211.

Delete this directory and the `[patch.crates-io]` entry once an iced release
carries the fix. On an iced upgrade before that, copy the new crate here and
apply the same change.
