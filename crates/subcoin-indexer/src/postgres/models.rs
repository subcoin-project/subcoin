use diesel::prelude::*;

#[derive(Queryable, Selectable)]
#[diesel(table_name = crate::postgres::schema::addresses)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Address {
    pub id: i32,
    pub address: String,
    pub current_btc_balance: Option<i64>
}
