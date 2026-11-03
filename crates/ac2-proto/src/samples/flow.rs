//! Flow control on the data socket: one of every client message and notice.

use crate::flow::{DataRequest, DataUp, FlowNotice};
use crate::topic::{Stream, Subscription, Topic};
use crate::units::MeasId;

/// One of every message a client sends on its data DEALER.
pub fn data_requests() -> Vec<DataRequest> {
    let tf = Topic::Data {
        meas: MeasId(3),
        stream: Stream::Tf,
    };
    [
        DataUp::Window { bytes: 65_536 },
        DataUp::Subscribe {
            sub: Subscription::AllData,
        },
        DataUp::Subscribe {
            sub: Subscription::Meas(MeasId(12)),
        },
        DataUp::Subscribe {
            sub: Subscription::InputMeters,
        },
        DataUp::Subscribe {
            sub: Subscription::Topic(Topic::Evt),
        },
        DataUp::Unsubscribe {
            sub: Subscription::Topic(tf),
        },
        DataUp::Keyframe { topic: tf },
        DataUp::Credit {
            bytes: 9_000,
            events: 3,
        },
    ]
    .into_iter()
    .map(DataRequest::new)
    .collect()
}

/// Every notice.
pub fn notices() -> Vec<FlowNotice> {
    vec![FlowNotice::ResyncRequired, FlowNotice::Rejoin]
}
