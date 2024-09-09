use clap::Parser;
use sc_cli::{
    Cors, RpcEndpoint, RpcMethods, RPC_DEFAULT_MAX_CONNECTIONS, RPC_DEFAULT_MAX_REQUEST_SIZE_MB,
    RPC_DEFAULT_MAX_RESPONSE_SIZE_MB, RPC_DEFAULT_MAX_SUBS_PER_CONN,
    RPC_DEFAULT_MESSAGE_CAPACITY_PER_CONN,
};
use sc_service::config::{IpNetwork, RpcBatchRequestConfig};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;

/// RPC parameters extracted from the upstream RunCmd.
#[derive(Debug, Clone, Parser)]
pub struct RpcParams {
    /// Listen to all RPC interfaces (default: local).
    ///
    /// Not all RPC methods are safe to be exposed publicly.
    ///
    /// Use an RPC proxy server to filter out dangerous methods. More details:
    /// <https://docs.substrate.io/build/remote-procedure-calls/#public-rpc-interfaces>.
    ///
    /// Use `--unsafe-rpc-external` to suppress the warning if you understand the risks.
    #[arg(long)]
    pub rpc_external: bool,

    /// Listen to all RPC interfaces.
    ///
    /// Same as `--rpc-external`.
    #[arg(long)]
    pub unsafe_rpc_external: bool,

    /// RPC methods to expose.
    #[arg(
		long,
		value_name = "METHOD SET",
		value_enum,
		ignore_case = true,
		default_value_t = RpcMethods::Auto,
		verbatim_doc_comment
	)]
    pub rpc_methods: RpcMethods,

    /// RPC rate limiting (calls/minute) for each connection.
    ///
    /// This is disabled by default.
    ///
    /// For example `--rpc-rate-limit 10` will maximum allow
    /// 10 calls per minute per connection.
    #[arg(long)]
    pub rpc_rate_limit: Option<NonZeroU32>,

    /// Disable RPC rate limiting for certain ip addresses.
    ///
    /// Each IP address must be in CIDR notation such as `1.2.3.4/24`.
    #[arg(long, num_args = 1..)]
    pub rpc_rate_limit_whitelisted_ips: Vec<IpNetwork>,

    /// Trust proxy headers for disable rate limiting.
    ///
    /// By default the rpc server will not trust headers such `X-Real-IP`, `X-Forwarded-For` and
    /// `Forwarded` and this option will make the rpc server to trust these headers.
    ///
    /// For instance this may be secure if the rpc server is behind a reverse proxy and that the
    /// proxy always sets these headers.
    #[arg(long)]
    pub rpc_rate_limit_trust_proxy_headers: bool,

    /// Set the maximum RPC request payload size for both HTTP and WS in megabytes.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_REQUEST_SIZE_MB)]
    pub rpc_max_request_size: u32,

    /// Set the maximum RPC response payload size for both HTTP and WS in megabytes.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_RESPONSE_SIZE_MB)]
    pub rpc_max_response_size: u32,

    /// Set the maximum concurrent subscriptions per connection.
    #[arg(long, default_value_t = RPC_DEFAULT_MAX_SUBS_PER_CONN)]
    pub rpc_max_subscriptions_per_connection: u32,

    /// Specify JSON-RPC server TCP port.
    #[arg(long, value_name = "PORT")]
    pub rpc_port: Option<u16>,

    /// EXPERIMENTAL: Specify the JSON-RPC server interface and this option which can be enabled
    /// several times if you want expose several RPC interfaces with different configurations.
    ///
    /// The format for this option is:
    /// `--experimental-rpc-endpoint" listen-addr=<ip:port>,<key=value>,..."` where each option is
    /// separated by a comma and `listen-addr` is the only required param.
    ///
    /// The following options are available:
    ///  • listen-addr: The socket address (ip:port) to listen on. Be careful to not expose the
    ///    server to the public internet unless you know what you're doing. (required)
    ///  • disable-batch-requests: Disable batch requests (optional)
    ///  • max-connections: The maximum number of concurrent connections that the server will
    ///    accept (optional)
    ///  • max-request-size: The maximum size of a request body in megabytes (optional)
    ///  • max-response-size: The maximum size of a response body in megabytes (optional)
    ///  • max-subscriptions-per-connection: The maximum number of subscriptions per connection
    ///    (optional)
    ///  • max-buffer-capacity-per-connection: The maximum buffer capacity per connection
    ///    (optional)
    ///  • max-batch-request-len: The maximum number of requests in a batch (optional)
    ///  • cors: The CORS allowed origins, this can enabled more than once (optional)
    ///  • methods: Which RPC methods to allow, valid values are "safe", "unsafe" and "auto"
    ///    (optional)
    ///  • optional: If the listen address is optional i.e the interface is not required to be
    ///    available For example this may be useful if some platforms doesn't support ipv6
    ///    (optional)
    ///  • rate-limit: The rate limit in calls per minute for each connection (optional)
    ///  • rate-limit-trust-proxy-headers: Trust proxy headers for disable rate limiting (optional)
    ///  • rate-limit-whitelisted-ips: Disable rate limiting for certain ip addresses, this can be
    /// enabled more than once (optional)  • retry-random-port: If the port is already in use,
    /// retry with a random port (optional)
    ///
    /// Use with care, this flag is unstable and subject to change.
    #[arg(
		long,
		num_args = 1..,
		verbatim_doc_comment,
		conflicts_with_all = &["rpc_external", "unsafe_rpc_external", "rpc_port", "rpc_cors", "rpc_rate_limit_trust_proxy_headers", "rpc_rate_limit", "rpc_rate_limit_whitelisted_ips", "rpc_message_buffer_capacity_per_connection", "rpc_disable_batch_requests", "rpc_max_subscriptions_per_connection", "rpc_max_request_size", "rpc_max_response_size"]
	)]
    pub experimental_rpc_endpoint: Vec<RpcEndpoint>,

    /// Maximum number of RPC server connections.
    #[arg(long, value_name = "COUNT", default_value_t = RPC_DEFAULT_MAX_CONNECTIONS)]
    pub rpc_max_connections: u32,

    /// The number of messages the RPC server is allowed to keep in memory.
    ///
    /// If the buffer becomes full then the server will not process
    /// new messages until the connected client start reading the
    /// underlying messages.
    ///
    /// This applies per connection which includes both
    /// JSON-RPC methods calls and subscriptions.
    #[arg(long, default_value_t = RPC_DEFAULT_MESSAGE_CAPACITY_PER_CONN)]
    pub rpc_message_buffer_capacity_per_connection: u32,

    /// Disable RPC batch requests
    #[arg(long, alias = "rpc_no_batch_requests", conflicts_with_all = &["rpc_max_batch_request_len"])]
    pub rpc_disable_batch_requests: bool,

    /// Limit the max length per RPC batch request
    #[arg(long, conflicts_with_all = &["rpc_disable_batch_requests"], value_name = "LEN")]
    pub rpc_max_batch_request_len: Option<u32>,

    /// Specify browser *origins* allowed to access the HTTP & WS RPC servers.
    ///
    /// A comma-separated list of origins (protocol://domain or special `null`
    /// value). Value of `all` will disable origin validation. Default is to
    /// allow localhost and <https://polkadot.js.org> origins. When running in
    /// `--dev` mode the default is to allow all origins.
    #[arg(long, value_name = "ORIGINS")]
    pub rpc_cors: Option<Cors>,
}

impl RpcParams {
    pub(crate) fn rpc_cors(&self, is_dev: bool) -> sc_cli::Result<Option<Vec<String>>> {
        Ok(self
            .rpc_cors
            .clone()
            .unwrap_or_else(|| {
                if is_dev {
                    tracing::warn!("Running in --dev mode, RPC CORS has been disabled.");
                    Cors::All
                } else {
                    Cors::List(vec![
                        "http://localhost:*".into(),
                        "http://127.0.0.1:*".into(),
                        "https://localhost:*".into(),
                        "https://127.0.0.1:*".into(),
                        "https://polkadot.js.org".into(),
                    ])
                }
            })
            .into())
    }

    pub(crate) fn rpc_addr(
        &self,
        default_listen_port: u16,
    ) -> sc_cli::Result<Option<Vec<RpcEndpoint>>> {
        if !self.experimental_rpc_endpoint.is_empty() {
            for endpoint in &self.experimental_rpc_endpoint {
                // Technically, `0.0.0.0` isn't a public IP address, but it's a way to listen on
                // all interfaces. Thus, we consider it as a public endpoint and warn about it.
                if endpoint.rpc_methods == RpcMethods::Unsafe && endpoint.is_global()
                    || endpoint.listen_addr.ip().is_unspecified()
                {
                    tracing::warn!(
                        "It isn't safe to expose RPC publicly without a proxy server that filters \
						 available set of RPC methods."
                    );
                }
            }

            return Ok(Some(self.experimental_rpc_endpoint.clone()));
        }

        let (ipv4, ipv6) = rpc_interface(
            self.rpc_external,
            self.unsafe_rpc_external,
            self.rpc_methods,
            false, // TODO: miner
        )?;

        let is_dev = false;
        let cors = self.rpc_cors(is_dev)?;
        let port = self.rpc_port.unwrap_or(default_listen_port);

        Ok(Some(vec![
            RpcEndpoint {
                batch_config: self.rpc_batch_config()?,
                max_connections: self.rpc_max_connections,
                listen_addr: SocketAddr::new(std::net::IpAddr::V4(ipv4), port),
                rpc_methods: self.rpc_methods,
                rate_limit: self.rpc_rate_limit,
                rate_limit_trust_proxy_headers: self.rpc_rate_limit_trust_proxy_headers,
                rate_limit_whitelisted_ips: self.rpc_rate_limit_whitelisted_ips.clone(),
                max_payload_in_mb: self.rpc_max_request_size,
                max_payload_out_mb: self.rpc_max_response_size,
                max_subscriptions_per_connection: self.rpc_max_subscriptions_per_connection,
                max_buffer_capacity_per_connection: self.rpc_message_buffer_capacity_per_connection,
                cors: cors.clone(),
                retry_random_port: true,
                is_optional: false,
            },
            RpcEndpoint {
                batch_config: self.rpc_batch_config()?,
                max_connections: self.rpc_max_connections,
                listen_addr: SocketAddr::new(std::net::IpAddr::V6(ipv6), port),
                rpc_methods: self.rpc_methods,
                rate_limit: self.rpc_rate_limit,
                rate_limit_trust_proxy_headers: self.rpc_rate_limit_trust_proxy_headers,
                rate_limit_whitelisted_ips: self.rpc_rate_limit_whitelisted_ips.clone(),
                max_payload_in_mb: self.rpc_max_request_size,
                max_payload_out_mb: self.rpc_max_response_size,
                max_subscriptions_per_connection: self.rpc_max_subscriptions_per_connection,
                max_buffer_capacity_per_connection: self.rpc_message_buffer_capacity_per_connection,
                cors: cors.clone(),
                retry_random_port: true,
                is_optional: true,
            },
        ]))
    }

    pub(crate) fn rpc_batch_config(&self) -> sc_cli::Result<RpcBatchRequestConfig> {
        let cfg = if self.rpc_disable_batch_requests {
            RpcBatchRequestConfig::Disabled
        } else if let Some(l) = self.rpc_max_batch_request_len {
            RpcBatchRequestConfig::Limit(l)
        } else {
            RpcBatchRequestConfig::Unlimited
        };

        Ok(cfg)
    }
}

fn rpc_interface(
    is_external: bool,
    is_unsafe_external: bool,
    rpc_methods: RpcMethods,
    is_validator: bool,
) -> sc_cli::Result<(Ipv4Addr, Ipv6Addr)> {
    if is_external && is_validator && rpc_methods != RpcMethods::Unsafe {
        return Err(sc_cli::Error::Input(
            "--rpc-external option shouldn't be used if the node is running as \
			 a validator. Use `--unsafe-rpc-external` or `--rpc-methods=unsafe` if you understand \
			 the risks. See the options description for more information."
                .to_owned(),
        ));
    }

    if is_external || is_unsafe_external {
        if rpc_methods == RpcMethods::Unsafe {
            tracing::warn!(
                "It isn't safe to expose RPC publicly without a proxy server that filters \
				 available set of RPC methods."
            );
        }

        Ok((Ipv4Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED))
    } else {
        Ok((Ipv4Addr::LOCALHOST, Ipv6Addr::UNSPECIFIED))
    }
}
