//! Screen a create request against what Krate can actually do, before spending
//! three to five minutes and an AI budget writing the wrong app.
//!
//! # Why this exists
//!
//! Nothing in the authoring loop compares the finished app to the request. The
//! six `check-app` stages are all mechanical -- does it build, does it import
//! only `krate:*`, does it run, does it paint a frame -- and every one of them
//! passes on a mail-reader UI drawn over invented local data. So
//! `krate create "download my email and show me the unread ones"` spends
//! minutes and hands back a plausible app that can never read anyone's mail,
//! and the person is told it is ready.
//!
//! That is worse under MCP than under the CLI. Someone who typed the command
//! remembers what they asked for. A model that gets a green check will tell its
//! user their email app works.
//!
//! # The rule this module follows
//!
//! **A false refusal is worse than today's behaviour.** Refusing something
//! Krate could have built makes the product look incapable, and the person has
//! no way to argue. So the bar is certainty, not suspicion:
//!
//! - Match the impossible **action**, never the topic. "download my email" is
//!   impossible; "an email-style inbox UI" is a windowed list Krate builds
//!   well. "message my friends" is impossible; "a chat app" is a two-pane
//!   layout. The noun is not the problem -- reaching another person's device,
//!   another company's account, or the host OS is.
//! - Every rule carries **rescue phrases**. A request that says "local", "fake",
//!   "mock", "demo", "sample", "offline", or "pretend" is asking for the thing
//!   Krate can build, and is never refused however impossible the verb sounds.
//! - When in doubt, build it. Uncertainty routes to [`Verdict::Caveat`], which
//!   builds the app and tells the truth about what it does, rather than to a
//!   refusal.
//!
//! The matcher is deliberately narrow. It is meant to catch the handful of
//! requests that are certainly impossible, not to classify the space.

use serde::{Deserialize, Serialize};

/// What Krate can do about a request, decided before any AI is spent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum Verdict {
    /// Nothing in the request is out of reach. Author it.
    Buildable,
    /// Buildable, but part of what was asked cannot be real -- typically live
    /// data from a service the app cannot reach. Author it, and say plainly in
    /// the output what the app does and does not do, so nobody discovers it
    /// later.
    Caveat(Caveat),
    /// Krate certainly cannot serve this. Stop now with one plain sentence
    /// instead of spending minutes on a wrong app.
    Refuse(Refusal),
}

/// A refusal: the one-sentence reason, and what the person can ask for instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The stable identifier for this limit, for tests and machine readers.
    pub limit: Limit,
    /// One plain sentence naming why. No jargon, no apology.
    pub reason: String,
    /// The nearest thing Krate can build, phrased as a request the person can
    /// paste straight back in. A refusal without this reads as a dead end.
    pub instead: String,
}

/// An honest note attached to an app that is built anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caveat {
    pub limit: Limit,
    /// One plain sentence saying what the app will actually do.
    pub note: String,
}

/// The real limits, named once so a message and a test agree on the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Limit {
    /// Reading the host OS's own apps and libraries: Mail, Photos, Contacts,
    /// Messages, the system calendar. A Krate app runs in a sandbox and cannot
    /// see them.
    HostApp,
    /// Signing in to somebody else's service. There is no OAuth, no token
    /// store, and no browser to redirect through.
    ThirdPartyAccount,
    /// Delivering to, or reading from, another person's device. There is no
    /// server, and two Krate apps have no way to find each other.
    AnotherDevice,
    /// Running when the app is not open. Krate apps have no background
    /// execution and no scheduler.
    Background,
    /// Reaching the network at all. Possible only through a specific granted
    /// `net.connect:host:port`; used as a caveat, never as a refusal.
    Network,
}

/// Phrases that mean "I want the local, made-up version" -- which Krate builds
/// well. Any of these anywhere in the request disables every refusal rule.
///
/// This is the single most important defence against a false refusal: someone
/// who says "a chat UI mockup with fake conversations" gets their app.
const RESCUE: &[&str] = &[
    "mock",
    "mockup",
    "mock-up",
    "fake",
    "pretend",
    "dummy",
    "sample data",
    "sample conversations",
    "demo data",
    "made up",
    "made-up",
    "local only",
    "local-only",
    "locally only",
    "offline",
    "no network",
    "no internet",
    "without the internet",
    "on this computer only",
    "same computer",
    "ui only",
    "ui-only",
    "just the ui",
    "just the interface",
    "prototype",
    "wireframe",
    "simulate",
    "simulated",
    "seeded",
];

/// One screening rule: the phrases that make a request certainly impossible,
/// the limit it hits, and what to say.
struct Rule {
    limit: Limit,
    /// Phrases that must appear for the rule to fire. Each is a whole action,
    /// not a topic word -- "download my email", not "email".
    triggers: &'static [&'static str],
    reason: &'static str,
    instead: &'static str,
}

/// The refusal rules. Deliberately few. Every trigger here names an action that
/// no Krate app can perform under any capability grant, which is why it is safe
/// to stop without asking the AI.
const RULES: &[Rule] = &[
    Rule {
        limit: Limit::HostApp,
        triggers: &[
            "download my email",
            "download my emails",
            "read my email",
            "read my emails",
            "show me my email",
            "show my email",
            "my unread email",
            "my inbox",
            "check my email",
            "fetch my email",
            "my real email",
            "read my mail",
            "my photo library",
            "my photos library",
            "back up my photos",
            "backup my photos",
            "my camera roll",
            "read my contacts",
            "my address book",
            "my text messages",
            "read my messages",
            "my imessage",
            "my system calendar",
            "my mac's",
            "my macs ",
        ],
        reason: "a Krate app runs in a sandbox and cannot read the apps or libraries \
                 already on your computer, so it cannot get at your real mail, photos, \
                 contacts, or messages",
        instead: "an app that works on files you pick yourself, or on data you type in",
    },
    Rule {
        limit: Limit::ThirdPartyAccount,
        triggers: &[
            "my gmail",
            "my google account",
            "my calendar from google",
            "my google calendar",
            "my outlook",
            "my spotify",
            "spotify client",
            "a spotify app",
            "my twitter",
            "my x account",
            "post to twitter",
            "posts to my twitter",
            "post to my twitter",
            "my instagram",
            "my facebook",
            "my slack",
            "my notion",
            "my dropbox",
            "my icloud",
            "my youtube account",
            "my github account",
            "log in to my",
            "log into my",
            "sign in to my",
            "sign into my",
            "log in with google",
            "sign in with google",
        ],
        reason: "a Krate app cannot sign in to another company's account for you: there \
                 is no login flow, no browser to redirect through, and nowhere safe to \
                 keep the token",
        instead: "an app that works on a file you export from that service, or on data \
                  you paste in",
    },
    Rule {
        limit: Limit::AnotherDevice,
        triggers: &[
            "message my friends",
            "messaging my friends",
            "message my friend",
            "chat with my friends",
            "talk to my friends",
            "send a message to my",
            "send messages to my",
            "sync my files to my phone",
            "sync to my phone",
            "sync my files",
            "send to my phone",
            "share with my friends",
            "video call",
            "voice call",
            "multiplayer over the internet",
            "play online with",
            "another person's computer",
            "someone else's computer",
        ],
        reason: "there is no Krate server and no way for two Krate apps to find each \
                 other, so an app on your computer cannot reach another person's device",
        instead: "a two-player app on this one computer, or an app that reads and writes \
                  a file you share yourself",
    },
    Rule {
        limit: Limit::Background,
        triggers: &[
            "in the background",
            "runs in the background",
            "even when it is closed",
            "even when closed",
            "when the app is closed",
            "while i sleep",
            "every morning at",
            "wake me up at",
            "notify me when i am not",
        ],
        reason: "a Krate app only runs while it is open: there is no background service \
                 and no scheduler to wake it",
        instead: "an app that does the same work while it is open on screen",
    },
    Rule {
        limit: Limit::HostApp,
        triggers: &[
            "back up to the cloud",
            "backup to the cloud",
            "to the cloud",
            "upload to the cloud",
            "in the cloud",
        ],
        reason: "\"the cloud\" means an account on somebody's server, and a Krate app has \
                 no server of its own and cannot sign in to anyone else's",
        instead: "an app that saves into its own folder on this computer, or writes files \
                  you choose where to put",
    },
];

/// Phrases that mean live data from the internet. Not a refusal -- Krate does
/// reach granted hosts over HTTPS -- but a request that asks for live data
/// without naming a host usually ends up with seeded demo data, and the person
/// should be told that in the output rather than discovering it later.
const NETWORK_HINTS: &[&str] = &[
    "live weather",
    "from the internet",
    "current weather",
    "today's weather",
    "todays weather",
    "the latest news",
    "live news",
    "stock price",
    "stock prices",
    "share price",
    "exchange rate",
    "exchange rates",
    "live score",
    "live scores",
    "current price",
];

/// True when the request explicitly names a host or URL to talk to. Such a
/// request is fully buildable: the app declares `net.connect:host:port`, the
/// person grants it, and the data is real. No caveat is warranted.
fn names_a_host(lower: &str) -> bool {
    lower.contains("http://")
        || lower.contains("https://")
        || lower.contains(" url")
        || lower.contains("a url")
        || lower.contains("an api i")
        || lower.contains("api i give")
        || lower.contains("url i give")
        || lower.contains("endpoint i")
        || lower.contains(".com/")
        || lower.contains("api key i")
}

/// Screen a request. See the module docs for the rule this follows: certainty
/// to refuse, honesty to caveat, and build when unsure.
pub fn screen(request: &str) -> Verdict {
    let lower = request.to_lowercase();

    // The rescue check runs first and beats everything. Someone who said
    // "fake", "local-only", "mockup" or "offline" has already told us they want
    // the buildable version, and no trigger phrase should override that.
    if RESCUE.iter().any(|phrase| lower.contains(phrase)) {
        return Verdict::Buildable;
    }

    for rule in RULES {
        if rule.triggers.iter().any(|t| lower.contains(t)) {
            return Verdict::Refuse(Refusal {
                limit: rule.limit,
                reason: rule.reason.to_string(),
                instead: rule.instead.to_string(),
            });
        }
    }

    // Live data with no host named: buildable, but say what it will really do.
    if !names_a_host(&lower) && NETWORK_HINTS.iter().any(|hint| lower.contains(hint)) {
        return Verdict::Caveat(Caveat {
            limit: Limit::Network,
            note: "this app can only reach the internet if you name a host for it and \
                   grant that permission, so unless you asked for a specific address it \
                   will show built-in example data rather than live figures"
                .to_string(),
        });
    }

    Verdict::Buildable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the refusal out of a verdict, failing the test with the request
    /// text when it was not refused.
    fn refusal(request: &str) -> Refusal {
        match screen(request) {
            Verdict::Refuse(r) => r,
            other => panic!("expected a refusal for {request:?}, got {other:?}"),
        }
    }

    fn assert_buildable(request: &str) {
        match screen(request) {
            Verdict::Buildable => {}
            other => panic!("{request:?} must not be refused or caveated, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // The refusals. One case per limit, each with the reason it names.
    // ---------------------------------------------------------------

    #[test]
    fn refuses_reading_the_computers_own_mail() {
        let r = refusal("download my email and show me the unread ones");
        assert_eq!(r.limit, Limit::HostApp);
        assert!(r.reason.contains("sandbox"), "reason was: {}", r.reason);
        assert!(!r.instead.is_empty());
    }

    #[test]
    fn refuses_messaging_another_person() {
        let r = refusal("a chat app so I can message my friends");
        assert_eq!(r.limit, Limit::AnotherDevice);
        assert!(r.reason.contains("another person's device"));
    }

    #[test]
    fn refuses_backing_up_to_a_cloud_account() {
        let r = refusal("back up my photos to the cloud");
        assert_eq!(r.limit, Limit::HostApp);
    }

    #[test]
    fn refuses_a_third_party_service_client() {
        let r = refusal("a Spotify client");
        assert_eq!(r.limit, Limit::ThirdPartyAccount);
        assert!(r.reason.contains("sign in"));
    }

    #[test]
    fn refuses_reading_a_google_account() {
        let r = refusal("show me my calendar from Google");
        assert_eq!(r.limit, Limit::ThirdPartyAccount);
    }

    #[test]
    fn refuses_posting_to_a_social_account() {
        let r = refusal("an app that posts to my twitter account");
        assert_eq!(r.limit, Limit::ThirdPartyAccount);
    }

    #[test]
    fn refuses_syncing_to_another_device() {
        let r = refusal("sync my files to my phone");
        assert_eq!(r.limit, Limit::AnotherDevice);
    }

    #[test]
    fn refuses_background_execution() {
        let r = refusal("a reminder that pops up even when the app is closed");
        assert_eq!(r.limit, Limit::Background);
    }

    #[test]
    fn every_refusal_names_one_reason_and_an_alternative() {
        // A refusal that does not say what to ask for instead is a dead end,
        // and a refusal without a plain reason is just a "no".
        for request in [
            "download my email and show me the unread ones",
            "a chat app so I can message my friends",
            "back up my photos to the cloud",
            "a Spotify client",
            "show me my calendar from Google",
            "sync my files to my phone",
        ] {
            let r = refusal(request);
            assert!(!r.reason.is_empty(), "{request} had no reason");
            assert!(!r.instead.is_empty(), "{request} had no alternative");
            // One sentence: no full stop in the middle of the reason.
            assert!(
                !r.reason.trim_end_matches('.').contains(". "),
                "{request} reason is more than one sentence: {}",
                r.reason
            );
        }
    }

    // ---------------------------------------------------------------
    // The part that matters more: things Krate CAN build must not be
    // refused. A false refusal makes the product look incapable.
    // ---------------------------------------------------------------

    #[test]
    fn does_not_refuse_a_local_chat_mockup() {
        // The noun "chat" is not the problem; reaching another device is.
        assert_buildable("a chat UI mockup with fake conversations");
        assert_buildable("a chat app that shows sample conversations, local only");
        assert_buildable("a two-pane messaging interface with made-up messages");
    }

    #[test]
    fn does_not_refuse_a_local_note_app() {
        assert_buildable("a local-only note app");
        assert_buildable("a note taking app that saves my notes");
    }

    #[test]
    fn does_not_refuse_an_email_shaped_ui_that_is_not_real_mail() {
        assert_buildable("an inbox-style list UI with fake messages");
        assert_buildable("an email client mockup showing sample messages");
    }

    #[test]
    fn does_not_refuse_a_weather_app_that_names_its_host() {
        // A named host means the app declares net.connect and the data is real.
        assert_buildable("a weather app that fetches from a URL I give it");
        assert_buildable("show the forecast from https://api.example.com/weather");
    }

    #[test]
    fn does_not_refuse_a_photo_app_that_works_on_files_i_pick() {
        assert_buildable("an image viewer that opens a PNG I choose");
        assert_buildable("a photo gallery for images in a folder I pick");
    }

    #[test]
    fn does_not_refuse_a_local_two_player_game() {
        assert_buildable("a tic tac toe game against another person on the same computer");
        assert_buildable("a pong game for two players on one keyboard");
        assert_buildable("a connect four game for two players");
    }

    #[test]
    fn does_not_refuse_a_calendar_that_is_its_own() {
        // "my calendar from Google" is refused; a calendar of your own is not.
        assert_buildable("a monthly calendar I can add my own events to");
        assert_buildable("a birthday reminder list showing who is next");
    }

    #[test]
    fn does_not_refuse_vague_or_oversized_requests() {
        // Vague and too-big are not impossible. The AI should attempt them --
        // refusing here would be exactly the false refusal we must avoid.
        for request in [
            "something for my mornings",
            "something to help me focus",
            "an app that makes my day better",
            "something fun",
            "a thing to organise my life",
            "a spreadsheet",
            "a word processor",
            "a photo editor like photoshop",
            "a full email client",
            "a web browser",
        ] {
            if let Verdict::Refuse(r) = screen(request) {
                panic!("{request:?} is hard, not impossible, but was refused: {r:?}")
            }
        }
    }

    /// The whole buildable corpus, screened in one test. This is the real
    /// guard: if a future rule starts refusing ordinary requests, this fails.
    #[test]
    fn refuses_nothing_in_the_buildable_corpus() {
        // Every request from evidence/reliability/corpus.txt sections 1-85 --
        // the ones Krate is expected to build. None may be refused.
        const BUILDABLE: &[&str] = &[
            "a to-do list I can check things off in",
            "a note taking app that saves my notes",
            "a habit tracker with a row of days for each habit",
            "a countdown timer for a 25 minute work session",
            "a daily journal where I write one entry a day",
            "a shopping list I can add and remove items from",
            "a reading list of books with a read/unread mark",
            "a simple kanban board with three columns",
            "a meeting cost calculator: people, hourly rate, minutes",
            "a water intake tracker for the day",
            "a packing list for a trip with checkboxes",
            "a birthday reminder list showing who is next",
            "a tip calculator",
            "a unit converter for length and weight",
            "a currency-style percentage calculator for discounts",
            "a BMI calculator",
            "a loan repayment calculator showing the monthly payment",
            "a temperature converter between celsius and fahrenheit",
            "a calculator with buttons for the four operations",
            "a date difference calculator: how many days between two dates",
            "a fuel cost calculator for a trip",
            "a split-the-bill calculator for a group",
            "a compound interest calculator",
            "a cooking measurement converter between cups and grams",
            "a pace calculator for runners",
            "a screen resolution aspect ratio calculator",
            "a tic tac toe game against another person on the same computer",
            "a memory matching card game",
            "a snake game",
            "a number guessing game where I guess what the computer picked",
            "a rock paper scissors game against the computer",
            "a dice roller that rolls two dice",
            "a game of hangman with a built in word list",
            "a simple breakout game with a paddle and a ball",
            "a whack-a-mole game with a score",
            "a 2048 sliding tile game",
            "a minesweeper game on a small grid",
            "a pong game for two players on one keyboard",
            "a reaction time tester: click when the colour changes",
            "a connect four game for two players",
            "a maze game where I move a dot to the exit",
            "a simon says colour memory game",
            "a stopwatch with lap times",
            "a colour picker that shows me the hex code",
            "a random password generator",
            "a QR-style grid pattern generator from a short text",
            "a text case converter: upper, lower, title case",
            "a word and character counter for text I paste in",
            "a markdown preview that shows my headings and lists",
            "a JSON pretty printer",
            "a CSV viewer that shows a table",
            "a text file viewer with line numbers",
            "an image viewer that opens a PNG I choose",
            "a hex viewer for a small file",
            "a diff viewer that compares two blocks of text",
            "a clipboard history that remembers the last few things",
            "a file renamer preview: show what the new names would be",
            "a base64 encoder and decoder",
            "an expense tracker with a running total",
            "a mood tracker with a face for each day",
            "a workout log with sets and reps",
            "a plant watering schedule",
            "a car mileage log",
            "a step goal tracker for the week",
            "a periodic table reference I can click an element in",
            "a colour name reference showing common web colours",
            "a keyboard shortcut cheatsheet for my editor",
            "a unit prefix reference: kilo, mega, giga",
            "a country capitals quiz",
            "a times tables practice app for a child",
            "a chord chart reference for a guitar",
            "a metric conversion reference table",
            "a drawing pad where I can draw with the mouse",
            "a pixel art editor on a 16 by 16 grid",
            "a colour palette generator from one base colour",
            "a gradient maker showing two colours blending",
            "a simple drum machine with four sounds",
            "a metronome that ticks at a set tempo",
            "a random name generator for characters",
            "a haiku composer that counts my syllables",
            "a mandala pattern drawer",
            "a bouncing ball screensaver",
            "a starfield animation",
            "a spirograph drawing toy",
            "a fractal viewer showing the mandelbrot set",
        ];
        for request in BUILDABLE {
            if let Verdict::Refuse(r) = screen(request) {
                panic!("corpus request {request:?} was falsely refused: {r:?}");
            }
        }
    }

    // ---------------------------------------------------------------
    // The honest-output case.
    // ---------------------------------------------------------------

    #[test]
    fn live_data_without_a_host_is_built_with_an_honest_note() {
        // The corpus's "show me the live weather from the internet" is NOT
        // impossible -- Krate has TLS and reaches granted hosts. But without a
        // host named it will almost certainly ship demo data, and the person
        // should hear that from `krate create`, not discover it.
        match screen("show me the live weather from the internet") {
            Verdict::Caveat(c) => {
                assert_eq!(c.limit, Limit::Network);
                assert!(c.note.contains("example data"), "note was: {}", c.note);
            }
            other => panic!("expected a caveat, got {other:?}"),
        }
    }

    #[test]
    fn naming_a_host_removes_the_caveat() {
        // Once the person names where the data comes from, the app can really
        // fetch it, so there is nothing to warn about.
        assert_buildable("show me the live weather from https://api.example.com/now");
        assert_buildable("a news reader that fetches from a URL I give it");
    }

    #[test]
    fn rescue_phrases_beat_every_rule() {
        // The safety valve, checked directly: any impossible-sounding request
        // that says it wants the local/fake version is built.
        for request in [
            "download my email, but use fake messages",
            "a Spotify client mockup with sample data",
            "back up my photos to the cloud -- simulate it, offline",
            "message my friends, ui only",
        ] {
            assert_buildable(request);
        }
    }
}
