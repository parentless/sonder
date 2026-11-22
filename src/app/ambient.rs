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
        let glow = img(move |window: &mut Window, cx: &mut App| {
            cache.update(cx, |cache, cx| cache.artist_backdrop(&url, window, cx))
        })
        .absolute()
        .left(gpui_kit::relative(-0.15))
        .bottom(gpui_kit::relative(-0.25))
        .w(gpui_kit::relative(0.9))
        .h(gpui_kit::relative(1.))
        .object_fit(gpui_kit::ObjectFit::Cover)
        .opacity(0.5);
        base.child(glow)
            // Keeps text contrast whatever the cover looks like.
            .child(div().absolute().inset_0().bg(gpui_kit::linear_gradient(
                180.,
                gpui_kit::linear_color_stop(rgb(palette.canvas).opacity(0.92), 0.),
                gpui_kit::linear_color_stop(rgb(palette.canvas).opacity(0.35), 1.),
            )))
    }
}
