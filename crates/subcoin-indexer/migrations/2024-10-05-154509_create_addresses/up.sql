-- Latest balance of each BTC address.
CREATE TABLE addresses (
    id SERIAL PRIMARY KEY,
    address TEXT UNIQUE NOT NULL,
    current_btc_balance BIGINT NOT NULL DEFAULT 0
);
