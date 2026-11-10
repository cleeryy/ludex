-- Minimal Galaxy-shaped schema used by the achievement provider tests.
CREATE TABLE Achievements (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    achievementId TEXT NOT NULL,
    name          TEXT NOT NULL,
    description   TEXT,
    dateUnlocked  TEXT,
    visible       INTEGER NOT NULL DEFAULT 1,
    points        INTEGER NOT NULL DEFAULT 0
);
