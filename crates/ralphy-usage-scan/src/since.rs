//! The `since` lower bound that the scans apply to their interactive records
//! (ADR-0033 §6).

use crate::InteractiveRecord;

/// Drop the records whose `last_ts` is strictly before `since`. The filter is
/// payload economy, so an unparseable bound or record keeps the record.
pub(crate) fn retain_since(records: &mut Vec<InteractiveRecord>, since: Option<&str>) {
    let Some(since) = since else {
        return;
    };
    if let Ok((since_dt, _)) = ralphy_time::parse_rfc3339(since) {
        records.retain(|r| match ralphy_time::parse_rfc3339(&r.last_ts) {
            Ok((last, _)) => last >= since_dt,
            Err(_) => true, // never hide spend on a parse miss
        });
    }
}
