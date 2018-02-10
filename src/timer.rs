use std::rc::Rc;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures::task::Task;

trait ClockState {
    fn now(&self) -> Instant;
    fn add_activation(&mut self, task: Task, when: Instant);
}

pub struct ClockHandle {
    clock: Rc<RefCell<ClockState>>,
}

#[derive(Debug)]
struct Activation {
    when: Instant,
    task: Task,
}

struct MockClockState {
    current: Instant,
    activations: VecDeque<Activation>,
}

pub struct MockClock {
    state: Rc<RefCell<MockClockState>>,
}

fn insert_activation(list: &mut VecDeque<Activation>, when: Instant, task: Task) {
    let activation = Activation {
        when: when,
        task: task,
    };

    // Quick check if we need to append at the end.
    let mut append_at_end = false;
    match list.back() {
        Some(a) if a.when > when => (),
        _ => append_at_end = true,
    }
    if append_at_end {
        list.push_back(activation);
        return;
    }

    // Use binary search to find the right place.
    let mut i0 = 0;
    let mut i1 = list.len();
    while i1 - i0 > 1 {
        let i = (i0 + i1) / 2;
        if list[i].when < when { i0 = i } else { i1 = i }
    }
    list.insert(i1, activation)
}

impl ClockHandle {
    pub fn now(&self) -> Instant {
        self.clock.borrow().now()
    }
    pub(crate) fn add_activation(&self, task: Task, when: Instant) {
        self.clock.borrow_mut().add_activation(task, when)
    }
}

impl Clone for ClockHandle {
    fn clone(&self) -> ClockHandle {
        ClockHandle { clock: self.clock.clone() }
    }
}

impl ClockState for MockClockState {
    fn now(&self) -> Instant {
        self.current
    }
    fn add_activation(&mut self, task: Task, when: Instant) {
        insert_activation(&mut self.activations, when, task)
    }
}

impl MockClock {
    pub fn new(start_time: Instant) -> MockClock {
        let state = MockClockState {
            current: start_time,
            activations: VecDeque::new(),
        };
        MockClock { state: Rc::new(RefCell::new(state)) }
    }

    pub fn advance(&mut self, duration: Duration) {
        let mut state = self.state.borrow_mut();
        state.current = state.current + duration;

        loop {
            match state.activations.front() {
                Some(a) if a.when <= state.current => (),
                _ => return,
            }

            let activation = state.activations.pop_front().unwrap();
            activation.task.notify()
        }
    }

    pub fn handle(&self) -> ClockHandle {
        ClockHandle { clock: self.state.clone() }
    }
}
