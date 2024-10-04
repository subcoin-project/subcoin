// @generated automatically by Diesel CLI.

diesel::table! {
    addresses (id) {
        id -> Int4,
        address -> Text,
        current_btc_balance -> Nullable<Int8>,
    }
}
