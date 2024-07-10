use crate::{validate_outbound_services, PeerId};
use bitcoin::p2p::address::{AddrV2, AddrV2Message, Address};
use std::collections::HashSet;
use std::net::IpAddr;

/// Manages the addresses discovered in the network.
#[derive(Debug)]
pub struct AddressBook {
    discovered_addresses: HashSet<PeerId>,
    active_addresses: HashSet<PeerId>,
    failed_addresses: HashSet<PeerId>,
    ipv4_only: bool,
    max_addresses: usize,
    rng: fastrand::Rng,
}

impl AddressBook {
    /// Constructs a new instance of [`AddressBook`].
    pub fn new(ipv4_only: bool, max_addresses: usize) -> Self {
        Self {
            discovered_addresses: HashSet::new(),
            active_addresses: HashSet::new(),
            failed_addresses: HashSet::new(),
            ipv4_only,
            max_addresses,
            rng: fastrand::Rng::new(),
        }
    }

    /// Checks if the address book has reached the maximum number of addresses.
    pub fn has_max_addresses(&self) -> bool {
        self.discovered_addresses.len() >= self.max_addresses
    }

    /// Pops a random address from the discovered addresses and marks it as active.
    pub fn pop(&mut self) -> Option<PeerId> {
        let maybe_peer = self.rng.choice(self.discovered_addresses.clone());

        if let Some(peer) = maybe_peer {
            self.discovered_addresses.remove(&peer);
            self.active_addresses.insert(peer);
        }

        maybe_peer
    }

    pub fn note_failed_address(&mut self, peer_addr: PeerId) {
        self.active_addresses.remove(&peer_addr);
        self.failed_addresses.insert(peer_addr);
    }

    /// Adds multiple addresses (`Address`) to the address book.
    pub fn add_many(&mut self, from: PeerId, addresses: Vec<(u32, Address)>) -> usize {
        let mut added = 0;

        for (_timestamp, address) in addresses {
            if self.has_max_addresses() {
                break;
            }

            if validate_outbound_services(address.services).is_err() {
                continue;
            }

            if let Ok(addr) = address.socket_addr() {
                if self.should_add_address(from, addr) {
                    self.discovered_addresses.insert(addr);
                    added += 1;
                }
            }
        }

        added
    }

    /// Adds multiple addresses (`AddrV2Message`) to the address book.
    pub fn add_many_v2(&mut self, from: PeerId, addresses: Vec<AddrV2Message>) -> usize {
        let mut added = 0;

        for address in addresses {
            if self.has_max_addresses() {
                break;
            }

            if validate_outbound_services(address.services).is_err() {
                continue;
            }

            let addr = match address.addr {
                AddrV2::Ipv4(addr) => PeerId::new(IpAddr::V4(addr), address.port),
                AddrV2::Ipv6(addr) => PeerId::new(IpAddr::V6(addr), address.port),
                _ => {
                    continue;
                }
            };

            if self.should_add_address(from, addr) {
                self.discovered_addresses.insert(addr);
                added += 1;
            }
        }

        added
    }

    fn should_add_address(&self, from: PeerId, new_addr: PeerId) -> bool {
        if from == new_addr {
            return false;
        }

        // Skip IPv6 if in IPv4-only mode.
        if self.ipv4_only && new_addr.is_ipv6() {
            return false;
        }

        if self.failed_addresses.contains(&new_addr) {
            return false;
        }

        true
    }
}
