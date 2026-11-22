//! Ambient design: a soft glow behind the whole window, made from the
//! blurred cover of the playing track. It is only mounted while Ambient is
//! selected, so other designs never create, load or paint any of it.

use super::*;

pub(super) struct AmbientGlow {
    url: Option<String>,
    image_cache: Entity<image_cache::BoundedImageCache>,
    _player: Subscription,
}

impl AmbientGlow {
    pub(super) fn new(player: &Entity<player::Player>, cx: &mut Context<Self>) -> Self {
        let url = Self::current_url(player, cx);
        let player_handle = player.clone();
        let subscription = cx.observe(player, move |this, _, cx| {
            // The player notifies on every clock tick; repaint only when the
            // cover changes.
            let url = Self::current_url(&player_handle, cx);
            if url != this.url {
                this.url = url;
                cx.notify();
            }
        });
        Self {
            url,
            image_cache: image_cache::BoundedImageCache::new(cx),
            _player: subscription,
        }
    }

    fn current_url(player: &Entity<player::Player>, cx: &App) -> Option<String> {
        player
            .read(cx)
            .shared_now_playing()
            .and_then(|track| track.artwork_url.clone())
    }
}

impl Render for AmbientGlow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = appearance::Appearance::palette(cx);
        let base = div().absolute().inset_0().overflow_hidden();
        // The dark base is already the window colour; light mode stays flat.
        let Some(url) = self.url.clone().filter(|_| palette.dark) else {
            return base;
        };
        let cache = self.image_cache.clone();
        // The glow sits low on the left and fades out to the right and the
        // top, so no hard edge of the cover reaches the page or its text.
        let glow = img(move |window: &mut Window, cx: &mut App| {
            cache.update(cx, |cache, cx| cache.artist_backdrop(&url, window, cx))
        })
        .absolute()
        .inset_0()
        .size_full()
        .object_fit(gpui_kit::ObjectFit::Cover)
        .opacity(0.42);
        let canvas = rgb(palette.canvas);
        base.child(
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .w(gpui_kit::relative(0.7))
                .h(gpui_kit::relative(0.8))
                .overflow_hidden()
                .child(glow)
                // Fade across, then up: the cover's own border never shows.
                .child(div().absolute().inset_0().bg(gpui_kit::linear_gradient(
                    90.,
                    gpui_kit::linear_color_stop(canvas.opacity(0.), 0.),
                    gpui_kit::linear_color_stop(canvas, 0.85),
                )))
                .child(div().absolute().inset_0().bg(gpui_kit::linear_gradient(
                    180.,
                    gpui_kit::linear_color_stop(canvas, 0.),
                    gpui_kit::linear_color_stop(canvas.opacity(0.), 0.7),
                ))),
        )
    }
}
