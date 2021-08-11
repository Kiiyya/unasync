//! # Un-async
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
//! You may have to wrap a foreign type (i.e. not from your own crate) in a newtype, and then
//! implmement [`UnSync`] for that instead of for the foreign type directly.
//!
//! # Example
//! Suppose you have the following `Counter` struct which is `!`[`Send`] and/or `!`[`Sync`]:
//! ```
//! # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
//! # use futures::future::join_all;
//! struct Counter {
//!     // PhantomData<*const ()> to effectively impl !Send and !Sync
//!     _private: PhantomData<*const ()>,
//!
//!     counter: isize,
//! }
//! ```
//! Then you implement [`UnSync`] like so, using your own `Message` enum, as follows.
//! You *can* use an enum, but you don't have to.
//! ```rust
//! # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
//! # use futures::future::join_all;
//! # struct Counter {
//! #     _private: PhantomData<*const ()>,
//! #     counter: isize,
//! # }
//! # impl Counter {
//! #     fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
//! # }
//! use unasync::{UnAsync, UnSync};
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
//!
//! Then, finally, you can use `Counter` indirectly using `UnAsync<Counter>` like so:
//! ```rust
//! # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
//! # use futures::future::join_all;
//! # use unasync::{UnAsync, UnSync};
//! # struct Counter {
//! #     _private: PhantomData<*const ()>,
//! #     counter: isize,
//! # }
//! # impl Counter {
//! #     fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
//! # }
//! # enum Message {
//! #     Get,
//! #     Add(isize),
//! # }
//! # impl UnSync for Counter {
//! #     type E = Infallible; // Error type
//! #     type Request = Message;
//! #     type Response = isize;
//! #     fn create() -> Result<Self, Self::E> {
//! #         Ok(Counter::new())
//! #     }
//! #     fn process(&mut self, request: Self::Request) -> Self::Response {
//! #         match request {
//! #             Message::Get => {
//! #                 self.counter
//! #             },
//! #             Message::Add(n) => {
//! #                 self.counter += n;
//! #                 sleep(Duration::from_millis(10)); // heavy computation
//! #                 self.counter
//! #             },
//! #         }
//! #     }
//! # }
//! # #[tokio::main]
//! async fn main() {
//!    let un = UnAsync::<Counter>::new().await.expect("Counter failed to initialize");
//!    let response = un.query(Message::Add(12345)).await;
//!    assert_eq!(12345, response);
//! }
//! ```
//!
//! If you want fallible operations, set `type Response = Result<..., MyAwesomeErrorType>` instead.


use std::{convert::Infallible, sync::Arc};

use crossbeam::channel::{Receiver, Sender};
use tokio::{sync::oneshot, task::{JoinHandle, spawn_blocking}};

pub trait UnSync {
    /// Error type which may occur on creation.
    type E: Send + Sync + 'static;

    /// Request type. This will commonly be an enum, for example:
    /// ```
    /// enum Message {
    ///     Get,
    ///     Add(isize),
    /// }
    /// ```
    type Request: Send + Sync + 'static;

    /// Response type, can be anything you want.
    ///
    /// If you want fallible operations, set this to for example `Result<_, _>`.
    type Response: Send + Sync +  'static;

    /// Function called to create [`Self`], called by [`UnAsync::new`].
    /// If `create` fails, the error is propagated to [`UnAsync::new`].
    fn create() -> Result<Self, Self::E> where Self: Sized;

    /// Process one message. This function gets called by [`UnAsync::query`].
    /// This function runs at most once at a time, hence you get `&mut self`.
    ///
    /// # Example
    /// ```rust
    /// # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
    /// # use futures::future::join_all;
    /// # use unasync::{UnAsync, UnSync};
    /// # struct Counter {
    /// #     _private: PhantomData<*const ()>,
    /// #     counter: isize,
    /// # }
    /// # impl Counter {
    /// #     fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
    /// # }
    /// # enum Message {
    /// #     Get,
    /// #     Add(isize),
    /// # }
    /// # impl UnSync for Counter {
    /// #     type E = Infallible; // Error type
    /// #     type Request = Message;
    /// #     type Response = isize;
    /// #     fn create() -> Result<Self, Self::E> {
    /// #         Ok(Counter::new())
    /// #     }
    /// fn process(&mut self, request: Self::Request) -> Self::Response {
    ///     match request {
    ///         Message::Get => {
    ///             self.counter
    ///         },
    ///         Message::Add(n) => {
    ///             self.counter += n;
    ///             sleep(Duration::from_millis(10)); // heavy computation
    ///             self.counter
    ///         },
    ///     }
    /// }
    /// # }
    /// ```
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
    sender: Sender<Request<T::Request, T::Response>>,

    /// The [`JoinHandle`] of the worker thread started by tokio.
    jh: Option<Arc<JoinHandle<()>>>,
}

impl<T: UnSync> Clone for UnAsync<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            jh: self.jh.clone(),
        }
    }
}

impl<T> UnAsync<T>
    where
        T: UnSync<E = Infallible> + 'static,
{
    /// Create new `UnAsync`, by invoking `T::create`, and returning the error in case `T::create`
    /// fails.
    ///
    /// This doesn't need to be async, since no errors can happen on creation, and the error doesn't need
    /// to be sent back.
    pub fn new_infallible() -> Self {
        let (tx, rx) = crossbeam::channel::unbounded();

        // channel where we'll transfer the error in case creation fails
        let (init_tx, _) = oneshot::channel();

        let mut myself = Self {
            sender: tx,
            jh: None,
        };
        let jh = spawn_blocking(move || UnAsync::<T>::run(rx, init_tx));
        myself.jh = Some(Arc::new(jh));

        myself
    }
}

impl<T: UnSync + 'static> UnAsync<T> {
    /// Create new `UnAsync`, by invoking `T::create`, and returning the error in case `T::create`
    /// fails.
    pub async fn new() -> Result<Self, T::E> {
        let (tx, rx) = crossbeam::channel::unbounded();

        // channel where we'll transfer the error in case creation fails
        let (init_tx, init_rx) = oneshot::channel();

        let mut myself = Self {
            sender: tx,
            jh: None,
        };
        let jh = spawn_blocking(move || UnAsync::<T>::run(rx, init_tx));
        myself.jh = Some(Arc::new(jh));
        init_rx.await.unwrap()?; // propagate error

        Ok(myself)
    }

    /// Sends a request to the single worker thread, and then waits on the response asynchronously.
    ///
    /// # Example
    /// ```rust
    /// # use std::{convert::Infallible, marker::PhantomData, thread::sleep, time::Duration};
    /// # use futures::future::join_all;
    /// # use unasync::{UnAsync, UnSync};
    /// # struct Counter {
    /// #     _private: PhantomData<*const ()>,
    /// #     counter: isize,
    /// # }
    /// # impl Counter {
    /// #     fn new() -> Self { Self { _private: PhantomData, counter: 0 } }
    /// # }
    /// # enum Message {
    /// #     Get,
    /// #     Add(isize),
    /// # }
    /// # impl UnSync for Counter {
    /// #     type E = Infallible; // Error type
    /// #     type Request = Message;
    /// #     type Response = isize;
    /// #     fn create() -> Result<Self, Self::E> {
    /// #         Ok(Counter::new())
    /// #     }
    /// #     fn process(&mut self, request: Self::Request) -> Self::Response {
    /// #         match request {
    /// #             Message::Get => {
    /// #                 self.counter
    /// #             },
    /// #             Message::Add(n) => {
    /// #                 self.counter += n;
    /// #                 sleep(Duration::from_millis(10)); // heavy computation
    /// #                 self.counter
    /// #             },
    /// #         }
    /// #     }
    /// # }
    /// # #[tokio::main]
    /// # async fn main() {
    /// # let un = UnAsync::<Counter>::new().await.expect("Counter failed to initialize");
    /// let response = un.query(Message::Add(12345)).await;
    /// assert_eq!(12345, response);
    /// # }
    /// ```
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

    fn run(rx: Receiver<Request<T::Request, T::Response>>, tx: oneshot::Sender<Result<(), T::E>>) {
        let mut instance = match T::create() {
            Ok(instance) => {
                let _ = tx.send(Ok(()));
                instance
            },
            Err(err) => {
                let _ = tx.send(Err(err));
                return;
            },
        };

        // once all `Sender` halves are dropped, we will get an `Err(RecvError)`, which then
        // gracefully exits the loop
        while let Ok(Request { request, response_to }) = rx.recv() {
            let response = instance.process(request);

            // If the user who sent the query dropped their oneshot::Receiver, that is fine, just ignore the error then.
            let _ = response_to.send(response);
        }

        // println!("run: Graceful shutdown. kthxbai");
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
        let un = UnAsync::<Counter>::new().await.unwrap();

        assert_eq!(1, un.query(Op::Add(1)).await);
    }

    #[tokio::test]
    async fn it_has_no_race_condition() {
        let un = UnAsync::<Counter>::new().await.unwrap();

        const N: isize = 100;

        // start N many queries at once
        let mut jhs = Vec::new();
        for _ in 0..N {
            jhs.push(un.query(Op::Add(1)))
        }
        join_all(jhs).await;

        assert_eq!(N, un.query(Op::Get).await);
    }

    #[derive(Debug)]
    struct OhNo;

    impl UnSync for OhNo {
        type E = &'static str;
        type Request = ();
        type Response = ();

        fn create() -> Result<Self, Self::E> where Self: Sized {
            Err("Oh nooo!")
        }

        fn process(&mut self, _: Self::Request) -> Self::Response { }
    }

    #[tokio::test]
    async fn it_propagates_errors_on_creation() {
        let ohno = UnAsync::<OhNo>::new().await.unwrap_err();
        assert_eq!("Oh nooo!", ohno);
    }

    trait Tester<T: Send + Sync> { }
    impl Tester<UnAsync<Counter>> for () {

    }
}
