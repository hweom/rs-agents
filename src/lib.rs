extern crate futures;

mod timer;

use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use futures::{Async, AsyncSink, Poll};
use futures::future::Future;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::sync::mpsc::{Receiver, Sender};
use futures::task::current;

pub use timer::{ClockHandle, MockClock};

enum InputResult {
    Ready,
    Closed,
}

enum TimerResult {
    Ready,
    Closed,
}

enum OutputResult {
    Ready,
    NotReady,
    Closed,
}

pub enum TimerRun {
    Continue,
    Stop,
}

trait PollableInput<S> {
    fn poll(&mut self, &mut S) -> InputResult;
}

trait PollableOutput {
    fn poll(&mut self) -> OutputResult;
}

trait PollableTimer<S> {
    fn poll(&mut self, &mut S) -> TimerResult;
}

struct Input<S, T, I, E>
where
    for<'r> I: FnMut(&'r mut S, T),
    for<'r> E: FnMut(&'r mut S),
{
    receiver: Option<Receiver<T>>,
    on_item: I,
    on_end: E,
    phantom_data: PhantomData<S>,
}

impl<S, T, I, E> PollableInput<S> for Input<S, T, I, E>
where
    for<'r> I: std::ops::FnMut(&'r mut S, T),
    for<'r> E: std::ops::FnMut(&'r mut S),
{
    fn poll(&mut self, state: &mut S) -> InputResult {
        if let Some(ref mut r) = self.receiver {
            match r.poll() {
                Ok(Async::Ready(Some(v))) => (self.on_item)(state, v),
                Ok(Async::Ready(None)) => (self.on_end)(state),
                Ok(Async::NotReady) => (),
                Err(_) => (),
            }
            return InputResult::Ready;
        }
        InputResult::Closed
    }
}

struct OutputState<T> {
    sender: Option<Sender<T>>,
    send_in_progress: bool,
    buffer: VecDeque<T>,
}

impl<T> OutputState<T> {
    fn poll(&mut self) -> OutputResult {
        if let Some(ref mut s) = self.sender {
            if self.send_in_progress {
                // Try to finish the current send.
                match s.poll_complete() {
                    Ok(Async::Ready(_)) => self.send_in_progress = false,
                    Ok(Async::NotReady) => return OutputResult::NotReady,
                    Err(_) => self.send_in_progress = false,
                }
            }

            if !self.send_in_progress {
                // Initiate new send.
                match self.buffer.pop_front() {
                    Some(v) => {
                        match s.start_send(v) {
                            Ok(AsyncSink::Ready) => self.send_in_progress = true,
                            Ok(AsyncSink::NotReady(v)) => self.buffer.push_front(v),
                            Err(_) => (),
                        }
                    }
                    None => (),
                }
            }
            return OutputResult::Ready;
        }
        OutputResult::Closed
    }
}

pub struct Output<T> {
    state: Rc<RefCell<OutputState<T>>>,
}

impl<T> Output<T> {
    pub fn send(&mut self, value: T) {
        let mut s = self.state.borrow_mut();
        s.buffer.push_back(value);
        s.poll();
    }
}

impl<T> PollableOutput for Output<T> {
    fn poll(&mut self) -> OutputResult {
        self.state.borrow_mut().poll()
    }
}

struct Timer<S, F>
where
    for<'r> F: FnMut(&'r mut S) -> TimerRun,
{
    clock: ClockHandle,
    on_timer: F,
    on: bool,
    period: Duration,
    next_activation: Option<Instant>,
    phantom_data: PhantomData<S>,
}

impl<S, F> PollableTimer<S> for Timer<S, F>
where
    for<'r> F: FnMut(&'r mut S) -> TimerRun,
{
    fn poll(&mut self, state: &mut S) -> TimerResult {
        if !self.on {
            return TimerResult::Closed;
        }

        let now = self.clock.now();
        match self.next_activation {
            None => {
                let next = now + self.period;
                self.next_activation = Some(next);
                self.clock.add_activation(current(), next);
            }
            Some(mut next) => {
                if now >= next {
                    (self.on_timer)(state);
                    while now >= next {
                        next = next + self.period
                    }
                    self.next_activation = Some(next);
                    self.clock.add_activation(current(), next);
                }
            }
        }

        TimerResult::Ready
    }
}

pub struct Builder<S> {
    inputs: Vec<Box<PollableInput<S>>>,
    outputs: Vec<Box<PollableOutput>>,
    timers: Vec<Box<PollableTimer<S>>>,
}

impl<S: 'static> Builder<S> {
    pub fn new() -> Builder<S> {
        Builder {
            inputs: Vec::new(),
            outputs: Vec::new(),
            timers: Vec::new(),
        }
    }

    pub fn new_input<T: 'static, I: FnMut(&mut S, T) + 'static, E: FnMut(&mut S) + 'static>(
        &mut self,
        receiver: Receiver<T>,
        on_item: I,
        on_end: E,
    ) {
        self.inputs.push(Box::new(Input {
            receiver: Some(receiver),
            on_item: on_item,
            on_end: on_end,
            phantom_data: PhantomData,
        }));
    }

    pub fn new_output<T: 'static>(&mut self, sender: Sender<T>) -> Output<T> {
        let state = Rc::new(RefCell::new(OutputState {
            sender: Some(sender),
            send_in_progress: false,
            buffer: VecDeque::new(),
        }));
        self.outputs.push(Box::new(Output { state: state.clone() }));
        Output { state: state }
    }

    pub fn new_timer<F: FnMut(&mut S) -> TimerRun + 'static>(
        &mut self,
        clock: ClockHandle,
        period: Duration,
        on_timer: F,
    ) {
        self.timers.push(Box::new(Timer {
            clock: clock,
            on_timer: on_timer,
            on: true,
            period: period,
            next_activation: None,
            phantom_data: PhantomData,
        }));
    }

    pub fn finish(self, state: S) -> Agent<S> {
        Agent {
            inputs: self.inputs,
            outputs: self.outputs,
            timers: self.timers,
            state: state,
        }
    }
}

pub struct Agent<S> {
    inputs: Vec<Box<PollableInput<S>>>,
    outputs: Vec<Box<PollableOutput>>,
    timers: Vec<Box<PollableTimer<S>>>,
    state: S,
}

impl<S> Future for Agent<S> {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let mut finished = true;
        for o in self.outputs.iter_mut() {
            match o.poll() {
                OutputResult::NotReady => return Ok(Async::NotReady),
                OutputResult::Ready => (),
                OutputResult::Closed => (),
            }
        }

        for t in self.timers.iter_mut() {
            match t.poll(&mut self.state) {
                TimerResult::Ready => finished = false,
                TimerResult::Closed => (),
            }
        }

        for i in self.inputs.iter_mut() {
            match i.poll(&mut self.state) {
                InputResult::Ready => finished = false,
                InputResult::Closed => (),
            }
        }

        match finished {
            false => Ok(Async::NotReady),
            true => Ok(Async::Ready(())),
        }
    }
}
