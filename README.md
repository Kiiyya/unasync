# Un-async
Suppose you have some struct (e.g. `Counter`) which is `!`[`Send`] and/or `!`[`Sync`],
but you still want to use it from an async context.
This crate allows you to build wrappers around your type by implementing the [`UnSync`] trait,
and then sends requests and receives responses via message-passing channels internally.

You can then use `UnAsync<Counter>` as a `Sync`, `Send`, and `Clone` wrapper around your
`Counter`.
All cloned `UnAsync<Counter>` will refer to the same counter.

You may have to wrap a foreign type (i.e. not from your own crate) in a newtype, and then
implmement [`UnSync`] for that instead of for the foreign type directly.

If you want fallible operations, set `type Response = Result<..., MyAwesomeErrorType>` instead.

# Example
Suppose you have the following `Counter` struct which is `!`[`Send`] and/or `!`[`Sync`]:
```rust
struct Counter {
    // PhantomData<*const ()> to effectively impl !Send and !Sync
    _private: PhantomData<*const ()>,
    counter: isize,
}
```
Then you implement `UnSync`, using your own `Message` enum, as follows.
You *can* use an enum, but you don't have to.
```rust
use unasync::{UnAsync, UnSync};

enum Message {
    Get,
    Add(isize),
}

impl UnSync for Counter {
    type E = Infallible;
    type Request = Message;
    type Response = isize;
    fn create() -> Result<Self, Self::E> {
        Ok(Counter::new())
    }
    fn process(&mut self, request: Self::Request) -> Self::Response {
        match request {
            Message::Get => {
                self.counter
            },
            Message::Add(n) => {
                self.counter += n;
                sleep(Duration::from_millis(10)); // heavy computation
                self.counter
            },
        }
    }
}
```
Then, finally, you use `Counter` indirectly using `UnAsync<Counter>` like so:
```rust
async fn main() {
   let un = UnAsync::<Counter>::new().await.expect("Counter failed to initialize");
   let response = un.query(Message::Add(12345)).await;
   assert_eq!(12345, response);
}
```
