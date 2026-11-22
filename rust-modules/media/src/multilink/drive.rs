//! **The machine's hands**: what a transport is, and the one loop that moves what a transport
//! received into [`Engine`] and what the engine asks for out to the transport.

use super::engine::{Fx, Engine, Event, Fault, Micros, ReqId, Request};
use super::object::Head;

/// What a transport reports of a request, in this order: its head, its body in pieces, how it
/// ended.
pub enum Arrival {
    Head(ReqId, Head),
    Body(ReqId, Vec<u8>),
    Done(ReqId),
    Failed(ReqId, Fault),
}

/// Ranges of one object over connections bound to source addresses.
pub trait Transport {
    /// Send `request`, with `if_range` as its `If-Range`. False when it could not be sent at all.
    fn start(&mut self, request: &Request, if_range: Option<&str>) -> bool;
    /// Forget a request. Nothing of it is reported afterwards.
    fn cancel(&mut self, id: ReqId);
    /// Move every request as far as it goes without waiting, and report what arrived.
    fn poll(&mut self, arrived: &mut Vec<Arrival>);
}

/// The buffers one [`Driver`] reuses between turns.
#[derive(Default)]
pub struct Driver {
    fx: Vec<Fx>,
    arrived: Vec<Arrival>,
}

impl Driver {
    /// Put one event to the machine and carry out what it asks. A request it starts is moved at
    /// once: it goes onto the wire now, not when the caller next takes a turn.
    pub fn step<T: Transport>(&mut self, engine: &mut Engine, net: &mut T, now: Micros, event: Event<'_>, log: &mut dyn FnMut(&str)) {
        engine.step(now, event, &mut self.fx);
        if self.act(engine, net, now, log) {
            self.turn(engine, net, now, log);
        }
    }

    /// One turn at `now`: what the transport received goes in, time passes, what the machine
    /// asks goes out. Twice over when a request was started, so it is on the wire before the
    /// caller waits.
    pub fn turn<T: Transport>(&mut self, engine: &mut Engine, net: &mut T, now: Micros, log: &mut dyn FnMut(&str)) {
        for _ in 0..2 {
            net.poll(&mut self.arrived);
            for arrival in self.arrived.drain(..) {
                let event = match arrival {
                    Arrival::Head(id, head) => Event::Head { id, head },
                    Arrival::Body(id, bytes) => Event::Body { id, bytes },
                    Arrival::Done(id) => Event::Done { id },
                    Arrival::Failed(id, fault) => Event::Failed { id, fault },
                };
                engine.step(now, event, &mut self.fx);
            }
            engine.step(now, Event::Tick, &mut self.fx);
            if !self.act(engine, net, now, log) {
                break;
            }
        }
    }

    /// Carry out the effects in hand; whether a request was started.
    fn act<T: Transport>(&mut self, engine: &mut Engine, net: &mut T, now: Micros, log: &mut dyn FnMut(&str)) -> bool {
        let mut started = false;
        while !self.fx.is_empty() {
            let mut batch = std::mem::take(&mut self.fx);
            for effect in batch.drain(..) {
                match effect {
                    Fx::Start(request) => {
                        started = true;
                        if !net.start(&request, engine.if_range()) {
                            engine.step(now, Event::Failed { id: request.id, fault: Fault::Connect }, &mut self.fx);
                        }
                    }
                    Fx::Cancel(id) => net.cancel(id),
                    Fx::Log(line) => log(&line),
                }
            }
            if self.fx.is_empty() {
                self.fx = batch;
                break;
            }
        }
        started
    }
}
