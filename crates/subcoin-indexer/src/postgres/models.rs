use crate::postgres::schema::addresses;
use diesel::prelude::*;

#[derive(Queryable, Selectable)]
#[diesel(table_name = addresses)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Address {
    pub id: i32,
    pub address: String,
    pub current_btc_balance: i64,
}

// Define the struct for inserting a new address
#[derive(Debug, Insertable)]
#[diesel(table_name = addresses)]
pub struct NewAddress<'a> {
    pub address: &'a str,
    pub current_btc_balance: i64,
}
