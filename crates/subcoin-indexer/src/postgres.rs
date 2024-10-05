pub mod models;
pub mod schema;

use crate::postgres::models::{Address, NewAddress};
use crate::postgres::schema::addresses;
use crate::BalanceChanges;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use dotenvy::dotenv;

pub struct PostgresStore {
    connection: PgConnection,
}

impl PostgresStore {
    // Creates a new instance of [`PostgresStore`].
    pub fn new() -> Self {
        dotenv().ok();

        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

        Self {
            connection: PgConnection::establish(&database_url)
                .unwrap_or_else(|_| panic!("Error connecting to {database_url}")),
        }
    }

    // Write balance changes to the database.
    pub fn write_balance_changes(&mut self, balance_changes: BalanceChanges) -> QueryResult<()> {
        // Start a transaction to ensure atomicity of the entire batch process.
        self.connection.transaction(|conn| {
            for (address, amount) in balance_changes.to_increase {
                // Check if the address already exists in the database
                match query_address(conn, &address) {
                    Ok(existing_address) => {
                        // Address exists, update its balance
                        let new_balance =
                            existing_address.current_btc_balance.unwrap_or(0) + amount as i64;
                        diesel::update(addresses::table.find(existing_address.id))
                            .set(addresses::current_btc_balance.eq(new_balance))
                            .execute(conn)?;
                    }
                    Err(diesel::result::Error::NotFound) => {
                        // Address does not exist, insert it into the database
                        let address = address.to_string();
                        let new_address = NewAddress {
                            address: address.as_str(),
                            current_btc_balance: amount as i64,
                        };
                        diesel::insert_into(addresses::table)
                            .values(&new_address)
                            .execute(conn)?;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }

            for (address, amount) in balance_changes.to_decrease {
                // Check if the address exists in the database
                match query_address(conn, &address) {
                    Ok(existing_address) => {
                        // Address exists, update its balance by subtracting
                        let new_balance =
                            existing_address.current_btc_balance.unwrap_or(0) - amount as i64;
                        diesel::update(addresses::table.find(existing_address.id))
                            .set(addresses::current_btc_balance.eq(new_balance))
                            .execute(conn)?;
                    }
                    Err(err) => {
                        tracing::error!("Address {address} not found in the database: {err:?}");
                    }
                }
            }

            Ok(())
        })
    }
}

fn query_address(conn: &mut PgConnection, address: &bitcoin::Address) -> QueryResult<Address> {
    addresses::table
        .filter(addresses::address.eq(address.to_string()))
        .first::<Address>(conn)
}
