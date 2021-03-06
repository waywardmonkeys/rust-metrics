// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// PrometheusReporter aggregates metrics into metricsfamilies and passes them every second to the
// attached prometheus reporter at a regular basis.

extern crate prometheus_reporter;
extern crate protobuf;
use self::prometheus_reporter::PrometheusReporter as Pr;
use self::prometheus_reporter::promo_proto;

use std::time::Duration;
use std::thread;
use metrics::Metric;
use time;
use std::collections::HashMap;
use std::sync::mpsc;
use self::protobuf::repeated::RepeatedField;

struct PrometheusMetricEntry {
    name: &'static str,
    metric: Metric,
    labels: HashMap<String, String>,
}

// TODO perhaps we autodiscover the host and port
//
pub struct PrometheusReporter {
    reporter_name: &'static str,
    host_and_port: &'static str,
    tx: Option<mpsc::Sender<PrometheusMetricEntry>>,
}

impl PrometheusReporter {
    pub fn new(reporter_name: &'static str, host_and_port: &'static str) -> Self {
        PrometheusReporter {
            reporter_name: reporter_name,
            host_and_port: host_and_port,
            tx: None,
        }
    }

    pub fn add(&mut self,
               name: &'static str,
               metric: Metric,
               labels: HashMap<String, String>)
               -> Result<(), String> {
        // TODO return error
        match self.tx {
            Some(ref mut tx) => {
                match tx.send(PrometheusMetricEntry {
                    name: name,
                    metric: metric,
                    labels: labels,
                }) {
                    Ok(x) => Ok(x),
                    Err(y) => Err(format!("Unable to send {}", y)),
                }
            }
            None => Err(format!("Please start the reporter before trying to add to it")),
        }
    }

    pub fn start(&mut self, delay_ms: u64) {
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx);
        let host_and_port = self.host_and_port.clone();
        thread::spawn(move || {
            let mut prometheus_reporter = Pr::new(host_and_port);
            prometheus_reporter.start().unwrap();
            loop {
                prometheus_reporter.add(collect_to_send(&rx));
                thread::sleep(Duration::from_millis(delay_ms));
            }
        });
    }
}

fn to_repeated_fields_labels(labels: HashMap<String, String>)
                             -> RepeatedField<promo_proto::LabelPair> {
    let mut repeated_fields = Vec::new();
    // name/value is what they call it in the protobufs *shrug*
    for (name, value) in labels {
        let mut label_pair = promo_proto::LabelPair::new();
        label_pair.set_name(name);
        label_pair.set_value(value);
        repeated_fields.push(label_pair);
    }
    RepeatedField::from_vec(repeated_fields)
}

fn make_metric(metric: &Metric,
               labels: &HashMap<String, String>)
               -> (promo_proto::Metric, promo_proto::MetricType) {

    let mut pb_metric = promo_proto::Metric::new();
    let ts = time::now().to_timespec().sec;

    pb_metric.set_timestamp_ms(ts);
    pb_metric.set_label(to_repeated_fields_labels(labels.clone()));
    match *metric {
        Metric::Counter(ref x) => {
            let snapshot = x.snapshot();
            let mut counter = promo_proto::Counter::new();
            counter.set_value(snapshot.value as f64);
            pb_metric.set_counter(counter);
            (pb_metric, promo_proto::MetricType::COUNTER)
        }
        Metric::Gauge(ref x) => {
            let snapshot = x.snapshot();
            let mut gauge = promo_proto::Gauge::new();
            gauge.set_value(snapshot.value as f64);
            pb_metric.set_gauge(gauge);
            (pb_metric, promo_proto::MetricType::GAUGE)
        }
        Metric::Meter(_) => {
            pb_metric.set_summary(promo_proto::Summary::new());
            (pb_metric, promo_proto::MetricType::SUMMARY)

        }
        Metric::Histogram(_) => {
            pb_metric.set_histogram(promo_proto::Histogram::new());
            (pb_metric, promo_proto::MetricType::HISTOGRAM)
        }
    }
}

fn collect_to_send(metric_entries: &mpsc::Receiver<PrometheusMetricEntry>)
                   -> Vec<promo_proto::MetricFamily> {
    let mut entries_group = HashMap::<&'static str, Vec<PrometheusMetricEntry>>::new();

    // Group them by name TODO we should include tags and types in the grouping
    for entry in metric_entries {
        let name = entry.name;
        let mut entries = entries_group.remove(name).unwrap_or(vec![]);
        entries.push(entry);
        entries_group.insert(name, entries);
    }

    let mut families = Vec::new();
    for (name, metric_entries) in &entries_group {
        let formatted_metric = format!("{}_{}_{}", "application_name", name, "bytes");
        // TODO check for 0 length

        let ref e1: PrometheusMetricEntry = metric_entries[0];
        let (_, pb_metric_type) = make_metric(&e1.metric, &e1.labels);

        let mut family = promo_proto::MetricFamily::new();
        let mut pb_metrics = Vec::new();

        for metric_entry in metric_entries {
            // TODO maybe don't assume they have the same type
            let (pb_metric, _) = make_metric(&metric_entry.metric, &metric_entry.labels);
            pb_metrics.push(pb_metric);
        }

        family.set_name(String::from(formatted_metric));
        family.set_field_type(pb_metric_type);
        family.set_metric(RepeatedField::from_vec(pb_metrics));
        families.push(family);
    }
    families
}



#[cfg(test)]
mod test {
    use histogram::Histogram;
    use std::collections::HashMap;
    use metrics::{Counter, Gauge, Meter, Metric, StdCounter, StdGauge, StdMeter};
    use super::PrometheusReporter;

    #[test]
    fn meter() {
        let m = StdMeter::new();
        m.mark(100);

        let c = StdCounter::new();
        c.inc();

        let g = StdGauge::new();
        g.set(2);

        let mut h = Histogram::configure()
            .max_value(100)
            .precision(1)
            .build()
            .unwrap();

        h.increment_by(1, 1).unwrap();

        let mut reporter = PrometheusReporter::new("test", "0.0.0.0:80");
        reporter.start(1024);
        let labels = HashMap::new();
        reporter.add("meter1", Metric::Meter(m.clone()), labels.clone());
        reporter.add("counter1", Metric::Counter(c.clone()), labels.clone());
        reporter.add("gauge1", Metric::Gauge(g.clone()), labels.clone());
        reporter.add("histogram", Metric::Histogram(h), labels);
    }
}
