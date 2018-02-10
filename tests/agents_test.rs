extern crate agents;
extern crate futures;
extern crate tokio_core;

use std::time::{Duration, Instant};

use agents::*;
use futures::{Sink, Stream};
use futures::sync::mpsc::{channel, Receiver, Sender};
use tokio_core::reactor::Core;

struct Passthrough {
    output: Output<i32>,
}

impl Passthrough {
    fn new(receiver: Receiver<i32>, sender: Sender<i32>) -> Agent<Passthrough> {
        let mut builder = Builder::new();
        let out = builder.new_output::<i32>(sender);
        builder.new_input(
            receiver,
            |s: &mut Passthrough, v: i32| s.on_input(v),
            |s: &mut Passthrough| s.on_input_end(),
        );
        builder.finish(Passthrough { output: out })
    }

    fn on_input(&mut self, val: i32) {
        self.output.send(val);
    }

    fn on_input_end(&mut self) {}
}

#[test]
fn passthrough() {
    let (tx1, rx1) = channel(1);
    let (tx2, rx2) = channel(1);
    let c = Passthrough::new(rx1, tx2);

    let mut core = Core::new().unwrap();

    core.handle().spawn(c);

    core.run(tx1.send(42)).unwrap();

    let out = core.run(rx2.take(1).collect()).unwrap();
    assert_eq!(out, vec![42])
}

struct Periodic {
    output: Output<i32>,
    count: i32,
}

impl Periodic {
    fn new(clock: ClockHandle, sender: Sender<i32>) -> Agent<Periodic> {
        let mut builder = Builder::new();
        let out = builder.new_output::<i32>(sender);
        builder.new_timer(clock.clone(), Duration::new(1, 0), |s: &mut Periodic| {
            s.on_timer()
        });
        builder.finish(Periodic {
            output: out,
            count: 0,
        })
    }

    fn on_timer(&mut self) -> TimerRun {
        self.output.send(self.count);
        self.count = self.count + 1;
        TimerRun::Continue
    }
}

#[test]
fn periodic() {
    let mut clock = MockClock::new(Instant::now());
    let (tx, mut rx) = channel(1);
    let c = Periodic::new(clock.handle(), tx);

    let mut core = Core::new().unwrap();

    core.handle().spawn(c);
    core.turn(None); // Poll component once to let it schedule the timer.

    for i in 0..10 {
        clock.advance(Duration::new(1, 0));
        core.turn(None);
        let (v, new_rx) = core.run(rx.into_future()).unwrap();
        assert_eq!(i, v.unwrap());
        rx = new_rx;
    }
}
