//! Un-async
//!
//! Suppose you have some struct (e.g. `Counter`) which is `!`[`Send`] and/or `!`[`Sync`],
//! but you still want to use it from an async context.
//! This crate allows you to build wrappers around your type by implementing the [`UnSync`] trait,
//! and then sends requests and receives responses via message-passing channels internally.
//!
//! You can then use `UnAsync<Counter>` as a `Sync`, `Send`, and `Clone` wrapper around your
//! `Counter`.
//! All cloned `UnAsync<Counter>` will refer to the same counter.
//!
//! # Example
//! ```
//! # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
//! # use futures::future::join_all;
//! use unasync::{UnAsync, UnSync};
//!
//! struct Counter {
//!     // Private to prevent struct construction
//!     // PhantomData<*const ()> to effectively impl !Send and !Sync on stable
//!     _private: PhantomData<*const ()>,
//!
//!     counter: isize,
//! }
//!
//! impl Counter {
//!     fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
//! }
//!
//! enum Message {
//!     Get,
//!     Add(isize),
//! }
//!
//! impl UnSync for Counter {
//!     type E = Infallible;
//!     type Request = Message;
//!     type Response = isize;
//!
//!     fn create() -> Result<Self, Self::E> {
//!         Ok(Counter::new())
//!     }
//!
//!     fn process(&mut self, request: Self::Request) -> Self::Response {
//!         match request {
//!             Message::Get => {
//!                 self.counter
//!             },
//!             Message::Add(n) => {
//!                 self.counter += n;
//!                 sleep(Duration::from_millis(10)); // heavy computation
//!                 self.counter
//!             },
//!         }
//!     }
//! }
//! ```


use std::sync::{Arc, mpsc::{self, Receiver}};

use tokio::{sync::oneshot, task::{JoinHandle, spawn_blocking}};

pub trait UnSync {
    type E: Send + Sync + 'static;
    type Request: Send + Sync + 'static;
    type Response: Send + Sync +  'static;

    fn create() -> Result<Self, Self::E> where Self: Sized;

    #[must_use]
    fn process(&mut self, request: Self::Request) -> Self::Response;
}

struct Request<Request: Send + Sync, Response: Send + Sync> {
    request: Request,
    response_to: oneshot::Sender<Response>,
}

/// Un-async. For more info, refer to the module documentation.
///
/// The inner [`Thing`] is dropped when the last [`UnAsync`] is dropped.
/// Uses [`Arc`] internally so can be cloned freely, and will refer to the same single object.
#[derive(Debug)]
pub struct UnAsync<T: UnSync> {
    sender: mpsc::Sender<Request<T::Request, T::Response>>,

    /// The [`JoinHandle`] of the worker thread started by tokio.
    #[allow(clippy::type_complexity)]
    jh: Option<Arc<JoinHandle<Result<(), T::E>>>>,
}

impl<T: UnSync> Clone for UnAsync<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            jh: self.jh.clone(),
        }
    }
}

impl<T: UnSync + 'static> UnAsync<T> {
    pub async fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        // let (resp_tx, resp_rx) = oneshot::channel();

        let mut myself = Self {
            sender: tx,
            jh: None,
        };
        let jh = spawn_blocking(move || UnAsync::<T>::run(rx));
        myself.jh = Some(Arc::new(jh));

        myself
    }

    /// Sends a request to the single worker thread, and then waits on the response asynchronously.
    pub async fn query(&self, request: T::Request) -> T::Response {
        let (tx, rx) = oneshot::channel();
        let request = Request {
            request,
            response_to: tx,
        };

        // println!("query: sending request..");
        self.sender
            .send(request)
            // the only way for the worker thread to exit is when it paniced, which can only happen
            // when `process()` panicked. I hope.
            .expect("The worker thread is not reachable anymore. Did it panic? If not, report this unasync bug.");

        // println!("query: waiting on response..");
        rx.await
            .expect("The worker thread quit before responding to me. Did it panic? If not, report this unasync bug.")
    }

    fn run(rx: Receiver<Request<T::Request, T::Response>>) -> Result<(), T::E> {
        let mut instance = T::create()?;

        // once all `Sender` halves are dropped, we will get an `Err(RecvError)`, which then
        // gracefully exits the loop
        while let Ok(Request { request, response_to }) = rx.recv() {
            let response = instance.process(request);

            // If the user who sent the query dropped their oneshot::Receiver, that is fine, just ignore the error then.
            let _ = response_to.send(response);
        }

        // println!("run: Graceful shutdown. kthxbai");
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};

    use futures::future::join_all;

    use crate::{UnAsync, UnSync};

    struct Counter {
        // Private to prevent struct construction
        // PhantomData<*const ()> to effectively impl !Send and !Sync on stable
        _private: PhantomData<*const ()>,

        counter: isize,
    }

    impl Counter {
        fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
    }

    enum Op {
        Get,
        Add(isize),
    }

    impl UnSync for Counter {
        type E = Infallible;
        type Request = Op;
        type Response = isize;

        fn create() -> Result<Self, Self::E> {
            Ok(Counter::new())
        }

        fn process(&mut self, request: Self::Request) -> Self::Response {
            match request {
                Op::Get => {
                    self.counter
                },
                Op::Add(n) => {
                    self.counter += n;
                    sleep(Duration::from_millis(10));
                    self.counter
                },
            }
        }
    }

    #[tokio::test]
    async fn it_works_basic() {
        let un = UnAsync::<Counter>::new().await;

        assert_eq!(1, un.query(Op::Add(1)).await);
    }

    #[tokio::test]
    async fn it_has_no_race_condition() {
        let un = UnAsync::<Counter>::new().await;

        const N: isize = 100;

        // start N many queries at once
        let mut jhs = Vec::new();
        for _ in 0..N {
            jhs.push(un.query(Op::Add(1)))
        }
        join_all(jhs).await;

        assert_eq!(N, un.query(Op::Get).await);
    }
}
