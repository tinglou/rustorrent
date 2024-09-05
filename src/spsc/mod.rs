use std::{
    cell::UnsafeCell,
    fmt::Debug,
    mem::MaybeUninit,
    sync::{
        atomic::{
            AtomicUsize,
            Ordering::{Acquire, Relaxed, Release},
        },
        Arc,
    },
};

const SHIFT: usize = (std::mem::size_of::<AtomicUsize>() * 8) - 1;
const CLOSED_BIT: usize = 1 << SHIFT;

struct Elem<T> {
    data: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Elem<T> {
    fn uninit() -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

pub enum PushError<T> {
    Full(T),
    Closed(T),
}

impl<T> Debug for PushError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Full(_) => write!(f, "PushError::Full(..)"),
            PushError::Closed(_) => write!(f, "PushError::Closed(..)"),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PopError {
    Empty,
    Closed,
}

pub struct Queue<T> {
    /// pop modify the head
    head: AtomicUsize,
    /// push modify the tail
    tail: AtomicUsize,
    buffer: Box<[Elem<T>]>,
    /// Read-only value
    mask_bit: usize,
}

pub struct Consumer<T> {
    queue: Arc<Queue<T>>,
}

impl<T> Consumer<T> {
    pub fn pop(&mut self) -> Result<T, PopError> {
        self.queue.pop()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

pub struct Producer<T> {
    queue: Arc<Queue<T>>,
}

impl<T> Producer<T> {
    pub fn push(&mut self, value: T) -> Result<(), PushError<T>> {
        self.queue.push(value)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn available(&self) -> usize {
        self.queue.available()
    }
}

impl<T: Copy> Producer<T> {
    pub fn push_slice(&self, slice: &[T]) -> Result<(), PushError<()>> {
        self.queue.push_slice(slice)
    }
}

unsafe impl<T> Send for Producer<T> {}
unsafe impl<T> Send for Consumer<T> {}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.queue.set_closed();
    }
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        self.queue.set_closed();
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // TODO: Optimize this
        while let Ok(value) = self.pop() {
            drop(value)
        }
    }
}

pub fn bounded<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    Queue::new(capacity)
}

impl<T> Queue<T> {
    fn new_queue(capacity: usize) -> Self {
        assert!(capacity > 0);

        let mut buffer = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buffer.push(Elem::uninit())
        }

        Queue {
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            buffer: buffer.into_boxed_slice(),
            mask_bit: (capacity + 1).next_power_of_two(),
        }
    }

    #[allow(clippy::new_ret_no_self)]
    fn new(capacity: usize) -> (Producer<T>, Consumer<T>) {
        let queue = Arc::new(Self::new_queue(capacity));

        (
            Producer {
                queue: Arc::clone(&queue),
            },
            Consumer { queue },
        )
    }

    fn set_closed(&self) {
        self.tail
            .fetch_update(Release, Relaxed, |tail| Some(tail | CLOSED_BIT))
            .unwrap();
    }

    fn len(&self) -> usize {
        let tail = self.tail.load(Acquire) & (self.mask_bit - 1);
        let head = self.head.load(Acquire) & (self.mask_bit - 1);

        if tail < head {
            (tail + self.buffer.len()) - head
        } else {
            tail - head
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn available(&self) -> usize {
        self.buffer.len() - self.len()
    }

    fn push(&self, elem: T) -> Result<(), PushError<T>> {
        let tail = self.tail.load(Relaxed);
        let head = self.head.load(Acquire);

        if tail & CLOSED_BIT != 0 {
            Err(PushError::Closed(elem))
        } else if head.wrapping_add(self.mask_bit) == tail {
            Err(PushError::Full(elem))
        } else {
            let index = tail & (self.mask_bit - 1);

            let data = self.buffer[index].data.get();
            unsafe {
                data.write(MaybeUninit::new(elem));
            }

            let next = if index + 1 < self.buffer.len() {
                tail + 1
            } else {
                (tail & !(self.mask_bit - 1)).wrapping_add(self.mask_bit)
            };

            self.tail.store(next, Release);

            Ok(())
        }
    }

    fn pop(&self) -> Result<T, PopError> {
        let head = self.head.load(Relaxed);
        let tail = self.tail.load(Acquire);

        if tail & !CLOSED_BIT == head {
            if tail & CLOSED_BIT != 0 {
                Err(PopError::Closed)
            } else {
                Err(PopError::Empty)
            }
        } else {
            let index = head & (self.mask_bit - 1);

            let data = self.buffer[index].data.get();
            let data = unsafe { data.read().assume_init() };

            let next = if index + 1 < self.buffer.len() {
                head + 1
            } else {
                (head & !(self.mask_bit - 1)).wrapping_add(self.mask_bit)
            };

            self.head.store(next, Release);

            Ok(data)
        }
    }
}

impl<T: Copy> Queue<T> {
    fn push_slice(&self, slice: &[T]) -> Result<(), PushError<()>> {
        let tail = self.tail.load(Relaxed);
        let head = self.head.load(Acquire);

        if tail & CLOSED_BIT != 0 {
            return Err(PushError::Closed(()));
        }

        let buffer_length = self.buffer.len();
        let slice_length = slice.len();
        let index = tail & (self.mask_bit - 1);

        let next_tail = if index + slice_length < buffer_length {
            tail + slice_length
        } else {
            (tail & !(self.mask_bit - 1))
                .wrapping_add(self.mask_bit)
                .wrapping_add((index + slice_length) % buffer_length)
        };

        if head.wrapping_add(self.mask_bit) < next_tail {
            Err(PushError::Full(()))
        } else {
            let buffer: &mut [T] = unsafe {
                #[allow(clippy::cast_ref_to_mut)]
                #[allow(invalid_reference_casting)]
                &mut *(&self.buffer[..] as *const [Elem<T>] as *mut [T])
            };

            if index + slice_length > buffer_length {
                let length = buffer_length - index;
                buffer[index..].copy_from_slice(&slice[..length]);

                buffer[..slice_length - length].copy_from_slice(&slice[length..]);
            } else {
                buffer[index..index + slice_length].copy_from_slice(slice);
            }

            self.tail.store(next_tail, Release);

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{PopError, PushError, Queue};

    #[test]
    fn simple() {
        let queue = Queue::new_queue(5);

        queue.push(1).unwrap();
        queue.push(2).unwrap();

        assert_eq!(queue.pop().unwrap(), 1);
        assert_eq!(queue.pop().unwrap(), 2);

        assert_eq!(queue.pop(), Err(PopError::Empty));
    }

    #[test]
    fn slices() {
        let queue = Queue::new_queue(5);

        queue.push_slice(&[1, 2, 3]).unwrap();
        queue.push(4).unwrap();
        queue.push_slice(&[5]).unwrap();

        assert_eq!(queue.pop().unwrap(), 1);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        assert_eq!(queue.pop().unwrap(), 4);
        assert_eq!(queue.pop().unwrap(), 5);

        assert_eq!(queue.pop(), Err(PopError::Empty));

        let queue = Queue::new_queue(5);

        queue.push_slice(&[1, 2]).unwrap();
        assert_eq!(queue.pop().unwrap(), 1);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop(), Err(PopError::Empty));

        queue.push_slice(&[1, 2, 3, 4]).unwrap();
        queue.push_slice(&[5]).unwrap();
        for n in 1..=5 {
            assert_eq!(queue.pop().unwrap(), n);
        }

        assert!(queue.is_empty());

        let queue = Queue::new_queue(5);
        queue.push_slice(&[1, 2, 3, 4, 5]).unwrap();
        for n in 1..=5 {
            assert_eq!(queue.pop().unwrap(), n);
        }

        let queue = Queue::new_queue(5);
        assert!(queue.push_slice(&[1, 2, 3, 4, 5, 6]).is_err());

        let queue = Queue::new_queue(5);
        queue.push(1).unwrap();
        queue.push_slice(&[2, 3, 4, 5]).unwrap();
        for n in 1..=5 {
            assert_eq!(queue.pop().unwrap(), n);
        }

        let queue = Queue::new_queue(5);
        queue.push(1).unwrap();
        queue.push_slice(&[2, 3, 4, 5, 6]).unwrap_err();
        assert_eq!(queue.pop().unwrap(), 1);
        assert!(queue.is_empty());

        let queue = Queue::new_queue(5);
        queue.push(1).unwrap();
        queue.pop().unwrap();
        queue.push_slice(&[2, 3, 4, 5, 6]).unwrap();
        queue.push(1).unwrap_err();
        for n in 2..=6 {
            assert_eq!(queue.pop().unwrap(), n);
        }

        assert_eq!(queue.available(), 5);

        let queue = Queue::new_queue(5);
        queue.push_slice(&[1, 2, 3, 4, 5]).unwrap();
        queue.push(6).unwrap_err();
        queue.push_slice(&[7]).unwrap_err();
        assert_eq!(queue.pop().unwrap(), 1);
        assert!(!queue.is_empty());

        let queue = Queue::new_queue(5);
        queue.push(0).unwrap();
        assert_eq!(queue.pop().unwrap(), 0);
        assert_eq!(queue.pop(), Err(PopError::Empty));
        queue.push_slice(&[1, 2, 3]).unwrap();
        queue.push_slice(&[4, 5, 6]).unwrap_err();
        assert_eq!(queue.pop().unwrap(), 1);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        queue.push_slice(&[7, 8, 9]).unwrap();
        assert_eq!(queue.pop().unwrap(), 7);
        assert_eq!(queue.pop().unwrap(), 8);
        assert_eq!(queue.pop().unwrap(), 9);
        queue.push_slice(&[10, 11, 12]).unwrap();
        assert_eq!(queue.pop().unwrap(), 10);
    }

    #[test]
    fn full() {
        let queue = Queue::new_queue(2);

        queue.push(1).unwrap();
        queue.push(2).unwrap();

        assert!(queue.push(3).is_err());
    }

    #[test]
    fn empty() {
        let queue = Queue::<usize>::new_queue(2);
        assert!(queue.pop().is_err());
    }

    #[test]
    fn seq() {
        let queue = Queue::new_queue(2);

        queue.push(1).unwrap();
        queue.push(2).unwrap();

        assert!(queue.push(3).is_err());

        assert_eq!(queue.pop().unwrap(), 1);
        queue.push(4).unwrap();

        assert!(queue.push(5).is_err());
        assert!(queue.push(6).is_err());

        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 4);

        assert!(queue.pop().is_err());
        assert!(queue.pop().is_err());

        queue.push(7).unwrap();
        assert_eq!(queue.pop().unwrap(), 7);
        queue.push(8).unwrap();
        queue.push(9).unwrap();

        assert!(queue.push(10).is_err());
        assert!(queue.push(11).is_err());

        assert_eq!(queue.pop().unwrap(), 8);
        assert_eq!(queue.pop().unwrap(), 9);
        assert!(queue.pop().is_err());
        assert!(queue.pop().is_err());
        assert!(queue.pop().is_err());

        queue.push(12).unwrap();
        queue.push(13).unwrap();

        assert_eq!(queue.pop().unwrap(), 12);
        assert_eq!(queue.pop().unwrap(), 13);

        queue.push(14).unwrap();
        assert_eq!(queue.pop().unwrap(), 14);
        queue.push(15).unwrap();
        assert_eq!(queue.pop().unwrap(), 15);
        queue.push(16).unwrap();
        assert_eq!(queue.pop().unwrap(), 16);

        queue.push(17).unwrap();
        queue.push(18).unwrap();
        assert!(queue.push(19).is_err());

        assert_eq!(queue.pop().unwrap(), 17);
        assert_eq!(queue.pop().unwrap(), 18);
        assert!(queue.pop().is_err());
    }

    #[test]
    fn closed() {
        let (mut sender, mut recv) = Queue::new(10);

        sender.push(10).unwrap();

        drop(sender);

        assert_eq!(recv.pop().unwrap(), 10);
        assert_eq!(recv.pop(), Err(PopError::Closed));
    }

    #[test]
    fn closed_recv() {
        let (mut sender, recv) = Queue::new(10);

        sender.push(1).unwrap();

        drop(recv);

        match sender.push(2) {
            Err(PushError::Closed(_)) => {}
            _ => panic!(),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Way too slow on miri
    fn threads() {
        for size in 1..=10 {
            let (mut sender, mut recv) = Queue::new(size);

            std::thread::spawn(move || {
                sender.push(1).unwrap();

                for n in 0..1_000_000 {
                    loop {
                        match sender.push(n) {
                            Ok(_) => break,
                            Err(PushError::Closed(_)) => panic!("closed"),
                            _ => {}
                        }
                    }
                }
            });

            while let Err(e) = recv.pop() {
                assert_eq!(e, PopError::Empty);
            }

            let mut last_value = 0;

            for n in 0..1_000_000 {
                loop {
                    match recv.pop() {
                        Ok(v) => {
                            assert_eq!(v, n, "value={} loop={} last_value={}", v, n, last_value);
                            last_value = v;
                            break;
                        }
                        Err(PopError::Closed) => panic!(),
                        _ => {}
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(recv.pop(), Err(PopError::Closed));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Way too slow on miri
    fn threads_slice() {
        for size in 5..=10 {
            let (mut sender, mut recv) = Queue::new(size);

            std::thread::spawn(move || {
                sender.push(1).unwrap();

                for n in (0..1_000_000).step_by(3) {
                    loop {
                        match sender.push_slice(&[n, n + 1, n + 2]) {
                            Ok(_) => break,
                            Err(PushError::Closed(_)) => panic!("closed"),
                            _ => {}
                        }
                    }
                }
            });

            while let Err(e) = recv.pop() {
                assert_eq!(e, PopError::Empty);
            }

            let mut last_value = 0;

            for n in 0..=1_000_001 {
                loop {
                    match recv.pop() {
                        Ok(v) => {
                            assert_eq!(v, n, "value={} loop={} last_value={}", v, n, last_value);
                            last_value = v;
                            break;
                        }
                        Err(PopError::Closed) => panic!(),
                        _ => {}
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
            assert_eq!(recv.pop(), Err(PopError::Closed));
        }
    }
}
