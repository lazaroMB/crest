//! The welcome tour: which step is showing, and where its card sits.
//!
//! The window paints this. Nothing here touches GPUI, so the sequence and
//! the placement can be checked without a window.

/// Version stored in preferences once the tour is finished or skipped.
/// A higher value shows the tour again.
pub const VERSION: u32 = 2;

/// One screen of the tour, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The centered welcome.
    Welcome,
    /// The New chat row.
    NewChat,
    /// The chats and projects list.
    Chats,
    /// The timeline.
    Timeline,
    /// The prompt.
    Prompt,
    /// The `/` command list above the prompt.
    Commands,
    /// The centered closing screen.
    Done,
}

impl Step {
    /// The five guided steps, for the "Step n of 5" line. The welcome and
    /// the closing screen have their own line.
    pub fn progress(self) -> Option<(u8, u8)> {
        match self {
            Self::NewChat => Some((1, 5)),
            Self::Chats => Some((2, 5)),
            Self::Timeline => Some((3, 5)),
            Self::Prompt => Some((4, 5)),
            Self::Commands => Some((5, 5)),
            Self::Welcome | Self::Done => None,
        }
    }

    /// Which part of the window this step points at.
    pub fn anchor(self) -> Option<Anchor> {
        match self {
            Self::NewChat => Some(Anchor::NewChat),
            Self::Chats => Some(Anchor::Chats),
            Self::Timeline => Some(Anchor::Timeline),
            Self::Prompt | Self::Commands => Some(Anchor::Prompt),
            Self::Welcome | Self::Done => None,
        }
    }

    /// Where the card prefers to sit relative to its anchor.
    pub fn side(self) -> Side {
        match self {
            Self::NewChat | Self::Chats => Side::Right,
            Self::Timeline => Side::Left,
            Self::Prompt | Self::Commands => Side::Above,
            Self::Welcome | Self::Done => Side::Center,
        }
    }

    /// The small line under the buttons, on the opening and closing screens.
    pub fn footer(self) -> Option<&'static str> {
        match self {
            Self::Welcome => Some("Welcome tutorial"),
            Self::Done => Some("Tutorial complete"),
            Self::NewChat | Self::Chats | Self::Timeline | Self::Prompt | Self::Commands => None,
        }
    }
}

/// A region the window measures and the tour highlights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The New chat row.
    NewChat,
    /// The chats sidebar.
    Chats,
    /// The timeline.
    Timeline,
    /// The prompt.
    Prompt,
}

/// What a button on the card does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Advance, or open the closing screen from the last step.
    Next,
    /// The previous step. Ignored on the welcome.
    Back,
    /// Close, and remember that the tour was seen.
    Exit,
    /// Back to the welcome, from the closing screen.
    Again,
    /// Close from the closing screen.
    Begin,
}

/// The words on one screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Copy {
    /// The title.
    pub title: &'static str,
    /// The sentence under it.
    pub body: &'static str,
}

impl Step {
    /// The title and the sentence.
    pub fn copy(self) -> Copy {
        match self {
            Self::Welcome => Copy {
                title: "Welcome!",
                body: "We'll show you the main features of the app, step by step.",
            },
            Self::NewChat => Copy {
                title: "Start a chat",
                body: "Click New chat to start a new conversation and organize your questions from scratch.",
            },
            Self::Chats => Copy {
                title: "Review your projects and conversations",
                body: "Open a project, continue an earlier conversation, or switch between sessions.",
            },
            Self::Timeline => Copy {
                title: "Explore the results",
                body: "This area shows analysis, answers, recommendations, and the main content the app generates.",
            },
            Self::Prompt => Copy {
                title: "Write your request here",
                body: "Use this bar to ask questions, request help, or describe tasks. Your interaction with the app starts here.",
            },
            Self::Commands => Copy {
                title: "Open commands with /",
                body: "Type / in this bar and the options appear. From there you can ask for help, change the theme, start research, or open this tutorial again.",
            },
            Self::Done => Copy {
                title: "All set!",
                body: "You know the main tools. You can start using the app with confidence.",
            },
        }
    }
}

/// Whether the tour should open by itself.
///
/// `seen` is the version stored in preferences. `blocked` is a login, a
/// model picker, an approval or another modal already on screen.
#[must_use]
pub fn should_auto_open(seen: Option<u32>, blocked: bool) -> bool {
    !blocked && seen.is_none_or(|version| version < VERSION)
}

/// The step after `action`, or `None` when the tour closes.
#[must_use]
pub fn after(step: Step, action: Action) -> Option<Step> {
    match action {
        Action::Exit | Action::Begin => None,
        Action::Again => Some(Step::Welcome),
        Action::Back => match step {
            Step::Welcome | Step::NewChat => Some(Step::Welcome),
            Step::Chats => Some(Step::NewChat),
            Step::Timeline => Some(Step::Chats),
            Step::Prompt => Some(Step::Timeline),
            Step::Commands => Some(Step::Prompt),
            Step::Done => Some(Step::Commands),
        },
        Action::Next => match step {
            Step::Welcome => Some(Step::NewChat),
            Step::NewChat => Some(Step::Chats),
            Step::Chats => Some(Step::Timeline),
            Step::Timeline => Some(Step::Prompt),
            Step::Prompt => Some(Step::Commands),
            Step::Commands | Step::Done => Some(Step::Done),
        },
    }
}

/// Whether closing with `action` should be remembered.
#[must_use]
pub fn remembers(action: Action) -> bool {
    matches!(action, Action::Exit | Action::Begin)
}

/// Where a card sits beside the highlighted region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// In the middle of the window.
    Center,
    /// To the right of the region.
    Right,
    /// To the left of the region.
    Left,
    /// Above the region.
    Above,
}

/// A rectangle in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Box2 {
    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn intersects(self, other: Box2) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// The dim rectangles around `hole`, so the hole itself stays clear.
/// A hole that fills the window leaves nothing to dim.
#[must_use]
pub fn shade(window: Box2, hole: Box2) -> Vec<Box2> {
    let hole = clamp(window, hole);
    let mut bands = Vec::new();
    if hole.y > 0.0 {
        bands.push(Box2 {
            x: 0.0,
            y: 0.0,
            w: window.w,
            h: hole.y,
        });
    }
    if hole.bottom() < window.h {
        bands.push(Box2 {
            x: 0.0,
            y: hole.bottom(),
            w: window.w,
            h: window.h - hole.bottom(),
        });
    }
    let mid_h = (hole.bottom() - hole.y).max(0.0);
    if hole.x > 0.0 {
        bands.push(Box2 {
            x: 0.0,
            y: hole.y,
            w: hole.x,
            h: mid_h,
        });
    }
    if hole.right() < window.w {
        bands.push(Box2 {
            x: hole.right(),
            y: hole.y,
            w: window.w - hole.right(),
            h: mid_h,
        });
    }
    bands
}

fn clamp(window: Box2, hole: Box2) -> Box2 {
    let x = hole.x.clamp(0.0, window.w);
    let y = hole.y.clamp(0.0, window.h);
    Box2 {
        x,
        y,
        w: hole.w.min(window.w - x).max(0.0),
        h: hole.h.min(window.h - y).max(0.0),
    }
}

/// Where the card of `side` goes. It stays inside `window` and, when there
/// is a hole, off that hole.
#[must_use]
pub fn place(window: Box2, hole: Option<Box2>, side: Side, card_w: f32, card_h: f32) -> Box2 {
    let gap = 16.0;
    let margin = 12.0;
    let (x, y) = match (side, hole) {
        (Side::Center, _) | (_, None) => ((window.w - card_w) / 2.0, (window.h - card_h) / 2.0),
        (Side::Right, Some(hole)) => (hole.right() + gap, hole.y),
        (Side::Left, Some(hole)) => (hole.x - gap - card_w, hole.y),
        (Side::Above, Some(hole)) => (hole.x, hole.y - gap - card_h),
    };
    let mut card = Box2 {
        x,
        y,
        w: card_w,
        h: card_h,
    };
    card.x = card
        .x
        .clamp(margin, (window.w - card_w - margin).max(margin));
    card.y = card
        .y
        .clamp(margin, (window.h - card_h - margin).max(margin));
    if let Some(hole) = hole
        && card.intersects(hole)
        && side != Side::Center
    {
        let below = hole.bottom() + gap;
        if below + card_h < window.h - margin {
            card.y = below;
        }
    }
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Box2 {
        Box2 {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 800.0,
        }
    }

    #[test]
    fn the_tour_opens_once_and_a_new_version_opens_again() {
        assert!(should_auto_open(None, false));
        assert!(!should_auto_open(Some(VERSION), false));
        assert!(
            !should_auto_open(None, true),
            "a login or a modal blocks it"
        );
        assert!(should_auto_open(Some(VERSION - 1), false));
    }

    #[test]
    fn next_and_back_walk_the_screens() {
        let mut step = Step::Welcome;
        for expected in [
            Step::NewChat,
            Step::Chats,
            Step::Timeline,
            Step::Prompt,
            Step::Commands,
            Step::Done,
        ] {
            step = after(step, Action::Next).unwrap();
            assert_eq!(step, expected);
        }
        step = after(step, Action::Back).unwrap();
        assert_eq!(step, Step::Commands);
        assert_eq!(after(Step::Commands, Action::Back), Some(Step::Prompt));
        assert_eq!(after(Step::Welcome, Action::Back), Some(Step::Welcome));
        assert_eq!(after(Step::Done, Action::Again), Some(Step::Welcome));
        assert_eq!(after(Step::Prompt, Action::Exit), None);
        assert_eq!(after(Step::Done, Action::Begin), None);
        assert!(remembers(Action::Exit));
        assert!(remembers(Action::Begin));
        assert!(!remembers(Action::Next));
        assert_eq!(Step::NewChat.progress(), Some((1, 5)));
        assert_eq!(Step::Commands.progress(), Some((5, 5)));
        assert_eq!(Step::Welcome.progress(), None);
        assert_eq!(Step::Welcome.copy().title, "Welcome!");
        assert_eq!(Step::Commands.anchor(), Some(Anchor::Prompt));
    }

    #[test]
    fn the_card_stays_in_the_window_and_off_the_highlight() {
        let hole = Box2 {
            x: 8.0,
            y: 40.0,
            w: 220.0,
            h: 36.0,
        };
        let card = place(window(), Some(hole), Side::Right, 360.0, 220.0);
        assert!(!card.intersects(hole));
        assert!(card.x >= 12.0 && card.right() <= window().w - 12.0);

        let prompt = Box2 {
            x: 240.0,
            y: 720.0,
            w: 1000.0,
            h: 48.0,
        };
        let above = place(window(), Some(prompt), Side::Above, 360.0, 220.0);
        assert!(!above.intersects(prompt));
        assert!(above.y >= 12.0);

        let narrow = Box2 {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 300.0,
        };
        let centered = place(narrow, None, Side::Center, 360.0, 220.0);
        assert!(centered.x >= 12.0);
        assert!(centered.right() <= narrow.w);

        let bands = shade(window(), hole);
        assert!(bands.iter().all(|band| !band.intersects(hole)));
        assert!(bands.len() >= 3);
    }
}
