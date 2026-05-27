# spire-slayer-architecture



src/

├── domain/     // cards, deck, run state — pure data

├── sim/        // playout engine + policies

├── metrics/    // each metric is a pure fn over state

├── tui/        // ratatui panels, no business logic

└── input/      // however state gets in (mod, OCR, manual)

