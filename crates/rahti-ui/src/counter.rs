//! A component with a server half, shipped by a library.
//!
//! This is the thing a component library could not do before: `bump` is an
//! ordinary `#[rpc]`, but nothing in the consuming app's source mentions it,
//! and the app's build never reads this file. The generated router therefore
//! cannot name it in a `match` the way it names a page's rpcs.
//!
//! It works because `#[rpc]` also submits an entry to the link-time registry
//! in `rahti::registry`, which the linker fills from every crate in the
//! binary. The page falls through to that registry for any name it does not
//! own itself.

use rahti::{Html, component, html, rpc};
use serde::Serialize;

/// A counter that counts on the server.
#[component]
pub fn Counter(label: &str) -> Html {
    html! {
        <div class="rahti-ui-counter">
            <h3 class="rahti-ui-counter-label">@{label}</h3>
            <button class="rahti-ui-button" onclick={bump()}>{caption}</button>
            <p class="rahti-ui-counter-tally">{tally}</p>

            <script>
                const [count, setCount] = pp.state(0);
                const [caption, setCaption] = pp.state("Count");
                const [tally, setTally] = pp.state("Nothing counted yet.");

                async function bump() {
                    const result = await pp.rpc("bump", { count });
                    setCount(result.count);
                    setTally(result.message);
                    setCaption("Count again");
                }
            </script>
        </div>
    }
}

/// What [`bump`] sends back.
#[derive(Serialize)]
pub struct Count {
    count: u32,
    message: String,
}

/// `pp.rpc("bump", { count })`, from the script above.
///
/// Written in a library, called from an app that never names it.
#[rpc]
pub async fn bump(count: u32) -> Count {
    let count = count.saturating_add(1);

    Count {
        message: match count {
            1 => "Counted once, by a library.".to_string(),
            n => format!("Counted {n} times, by a library."),
        },
        count,
    }
}
