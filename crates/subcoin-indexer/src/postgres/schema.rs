// @generated automatically by Diesel CLI.

diesel::table! {
    address_historical_balances (id) {
        id -> Int4,
        address_id -> Nullable<Int4>,
        block_height -> Int4,
        balance -> Int8,
    }
}

diesel::table! {
    addresses (id) {
        id -> Int4,
        address -> Text,
        current_btc_balance -> Int8,
    }
}

diesel::joinable!(address_historical_balances -> addresses (address_id));

diesel::allow_tables_to_appear_in_same_query!(
    address_historical_balances,
    addresses,
);
