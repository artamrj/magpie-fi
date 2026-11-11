-- Telegram: the "which song is this?" question can be a card (a Bot API 10.4
-- widget). 0: text or not asked, 1: a card waiting for a choice, 2: a card that
-- was answered (in the card, the chat or the app) and shows the outcome.
ALTER TABLE wanted ADD COLUMN review_card INTEGER NOT NULL DEFAULT 0;
