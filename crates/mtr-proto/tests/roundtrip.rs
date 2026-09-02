use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use mtr_proto::*;
use proptest::prelude::*;

fn arb_ip() -> impl Strategy<Value = IpAddr> {
    prop_oneof![
        any::<[u8; 4]>().prop_map(|o| IpAddr::V4(Ipv4Addr::from(o))),
        any::<[u8; 16]>().prop_map(|o| IpAddr::V6(Ipv6Addr::from(o))),
    ]
}

fn arb_protocol() -> impl Strategy<Value = Protocol> {
    prop_oneof![
        Just(Protocol::Icmp),
        Just(Protocol::Udp),
        Just(Protocol::Tcp),
        Just(Protocol::Sctp)
    ]
}

fn arb_params() -> impl Strategy<Value = ProbeParams> {
    (
        (
            arb_ip(),
            any::<bool>(),
            proptest::option::of("[a-z0-9]{1,8}"),
            arb_protocol(),
        ),
        (
            proptest::option::of(any::<u16>()),
            proptest::option::of(any::<u8>()),
            proptest::option::of(any::<u8>()),
            proptest::option::of(any::<u8>()),
        ),
        (
            proptest::option::of(any::<u32>()),
            proptest::option::of(any::<u16>()),
            proptest::option::of(any::<u16>()),
            proptest::option::of(any::<u32>()),
        ),
    )
        .prop_map(
            |(
                (target, has_local, local_device, protocol),
                (size, bit_pattern, tos, ttl),
                (timeout_s, port, local_port, mark),
            )| {
                ProbeParams {
                    target,
                    local_ip: has_local.then_some(target),
                    local_device,
                    protocol,
                    size,
                    bit_pattern,
                    tos,
                    ttl,
                    timeout_s,
                    port,
                    local_port,
                    mark,
                }
            },
        )
}

fn arb_mpls() -> impl Strategy<Value = Vec<MplsLabel>> {
    proptest::collection::vec(
        (any::<u32>(), any::<u8>(), any::<bool>(), any::<u8>()).prop_map(
            |(label, tc, bottom_of_stack, ttl)| MplsLabel {
                label,
                tc,
                bottom_of_stack,
                ttl,
            },
        ),
        0..=8,
    )
}

fn arb_response_kind() -> impl Strategy<Value = ResponseKind> {
    let bare = proptest::sample::select(vec![
        ResponseKind::NoReply,
        ResponseKind::UnknownCommand,
        ResponseKind::ProbesExhausted,
        ResponseKind::PermissionDenied,
        ResponseKind::AddressInUse,
        ResponseKind::AddressNotAvailable,
        ResponseKind::NetworkDown,
        ResponseKind::HostDown,
        ResponseKind::NoRouteNetwork,
        ResponseKind::NoRouteHost,
        ResponseKind::WaitTcpResponseTimeout,
        ResponseKind::CommandParseError,
        ResponseKind::CommandBufferOverflow,
    ]);
    prop_oneof![
        (
            prop_oneof![
                Just(ProbeResult::Reply),
                Just(ProbeResult::TtlExpired),
                Just(ProbeResult::NoRouteHost)
            ],
            arb_ip(),
            any::<u32>(),
            arb_mpls()
        )
            .prop_map(|(result, addr, rtt_us, mpls)| ResponseKind::Probe {
                result,
                addr,
                rtt_us,
                mpls
            }),
        "[a-z0-9.]{1,10}".prop_map(ResponseKind::FeatureSupport),
        proptest::option::of(prop_oneof![
            Just(InvalidReason::IpVersionNotSupported),
            Just(InvalidReason::ProtocolNotSupported)
        ])
        .prop_map(|reason| ResponseKind::InvalidArgument { reason }),
        proptest::option::of(any::<i64>())
            .prop_map(|errno| ResponseKind::UnexpectedError { errno }),
        bare,
    ]
}

proptest! {
    #[test]
    fn request_round_trips(token in any::<i32>(), p in arb_params()) {
        let r = Request { token, kind: RequestKind::SendProbe(p) };
        prop_assert_eq!(Request::parse(&r.encode()).unwrap(), r);
    }

    #[test]
    fn response_round_trips(token in any::<i32>(), kind in arb_response_kind()) {
        let r = Response { token, kind };
        prop_assert_eq!(Response::parse(&r.encode()).unwrap(), r);
    }

    #[test]
    fn parsers_never_panic(s in "\\PC{0,200}") {
        let _ = mtr_proto::tokenize::tokenize(&s);
        let _ = Request::parse(&s);
        let _ = Response::parse(&s);
    }
}
