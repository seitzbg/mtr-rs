//! Asynchronous PTR and origin-AS TXT lookups. Replaces the forked resolver of ui/dns.c and the
//! blocking `res_query()` of ui/asn.c with hickory-resolver. GPL-2.0-only.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use hickory_resolver::TokioResolver;
use hickory_resolver::proto::rr::{Name, RData, RecordType};
use tokio::sync::{Semaphore, mpsc};

use crate::asn::{self, AsnInfo};
use crate::names::{LookupResult, is_useful_hostname};

#[derive(Debug, Clone)]
pub struct ResolverConfig {
    pub provider4: String,
    pub provider6: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupRequest {
    Ptr(IpAddr),
    Asn(IpAddr),
}

/// Handle to the resolver worker: send requests on `tx`, receive results on `rx`.
pub struct Resolver {
    pub tx: mpsc::Sender<LookupRequest>,
    pub rx: mpsc::Receiver<LookupResult>,
}

const MAX_IN_FLIGHT: usize = 8;

impl Resolver {
    /// System-configured resolver (2 s timeout, 2 attempts) plus a worker task.
    pub fn start(cfg: ResolverConfig) -> Result<Resolver, String> {
        let mut builder =
            TokioResolver::builder_tokio().map_err(|e| format!("resolver init: {e}"))?;
        {
            let opts = builder.options_mut();
            opts.timeout = Duration::from_secs(2);
            opts.attempts = 2;
        }
        let resolver = Arc::new(
            builder
                .build()
                .map_err(|e| format!("resolver build: {e}"))?,
        );
        let cfg = Arc::new(cfg);
        let (req_tx, mut req_rx) = mpsc::channel::<LookupRequest>(1024);
        let (res_tx, res_rx) = mpsc::channel::<LookupResult>(1024);
        tokio::spawn(async move {
            let limit = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
            while let Some(req) = req_rx.recv().await {
                let Ok(permit) = limit.clone().acquire_owned().await else {
                    break;
                };
                let (resolver, cfg, res_tx) = (resolver.clone(), cfg.clone(), res_tx.clone());
                tokio::spawn(async move {
                    let result = match req {
                        LookupRequest::Ptr(ip) => LookupResult::Ptr {
                            addr: ip,
                            name: lookup_ptr(&resolver, ip).await,
                        },
                        LookupRequest::Asn(ip) => LookupResult::Asn {
                            addr: ip,
                            info: Some(lookup_asn(&resolver, &cfg, ip).await),
                        },
                    };
                    drop(permit);
                    let _ = res_tx.send(result).await;
                });
            }
        });
        Ok(Resolver {
            tx: req_tx,
            rx: res_rx,
        })
    }
}

/// Reverse lookup; `None` for NXDOMAIN, errors, or a useless name (dns.c:182).
pub async fn lookup_ptr(resolver: &TokioResolver, ip: IpAddr) -> Option<String> {
    let lookup = resolver
        .lookup(Name::from(ip), RecordType::PTR)
        .await
        .ok()?;
    lookup
        .answers()
        .iter()
        .find_map(|r| match &r.data {
            RData::PTR(p) => Some(p.to_string()),
            _ => None,
        })
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| is_useful_hostname(s))
}

/// First character-string of the first TXT record of `qname`.
async fn lookup_txt(resolver: &TokioResolver, qname: &str) -> Option<String> {
    match resolver.lookup(qname, RecordType::TXT).await {
        Ok(lookup) => lookup.answers().iter().find_map(|r| match &r.data {
            RData::TXT(t) => t
                .txt_data
                .first()
                .map(|b| String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }),
        Err(e) => {
            tracing::debug!("txt lookup {qname}: {e}");
            None
        }
    }
}

/// Origin AS record, then the AS name (spec §7.2). A failed origin lookup becomes `???`, which C
/// also caches after a failed `res_query()` (asn.c:197-201); a failed name lookup leaves `name: None`.
pub async fn lookup_asn(resolver: &TokioResolver, cfg: &ResolverConfig, ip: IpAddr) -> AsnInfo {
    // Deviation: the AS-name zone must match the provider `query_name` actually used, not
    // `ip.is_ipv6()` — a NAT64 address folds to the IPv4 zone (asn.c:329-332, 409-419).
    let provider = asn::query_provider(ip, &cfg.provider4, &cfg.provider6);
    let qname = asn::query_name(ip, &cfg.provider4, &cfg.provider6);
    let mut info = lookup_txt(resolver, &qname)
        .await
        .map(|t| asn::parse_txt(&t))
        .unwrap_or_else(AsnInfo::unknown);
    if let Some(q) = asn::as_name_query(&info, asn::name_zone(provider)) {
        info.name = lookup_txt(resolver, &q)
            .await
            .as_deref()
            .and_then(asn::parse_as_name);
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs real DNS; run with `MTR_E2E=1 cargo test -p mtr -- --ignored resolver`.
    #[tokio::test]
    #[ignore]
    async fn resolves_ptr_and_asn_for_a_public_address() {
        if std::env::var_os("MTR_E2E").is_none() {
            return;
        }
        let cfg = ResolverConfig {
            provider4: "origin.asn.cymru.com".into(),
            provider6: "origin6.asn.cymru.com".into(),
        };
        let mut r = Resolver::start(cfg).unwrap();
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        r.tx.send(LookupRequest::Ptr(ip)).await.unwrap();
        r.tx.send(LookupRequest::Asn(ip)).await.unwrap();
        let mut got_ptr = false;
        let mut got_asn = false;
        for _ in 0..2 {
            match tokio::time::timeout(Duration::from_secs(10), r.rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                LookupResult::Ptr { name, .. } => {
                    assert_eq!(name.as_deref(), Some("dns.google"));
                    got_ptr = true;
                }
                LookupResult::Asn { info, .. } => {
                    let info = info.unwrap();
                    assert!(info.field(0).contains("15169"));
                    assert!(
                        info.name.as_deref().unwrap_or("").contains("GOOGLE"),
                        "{info:?}"
                    );
                    got_asn = true;
                }
            }
        }
        assert!(got_ptr && got_asn);
    }
}
