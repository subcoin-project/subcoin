use crate::num::ScriptNum;
use crate::Error;
use std::ops::{Deref, DerefMut};

#[derive(Debug, Default, PartialEq, Clone)]
pub struct Stack<T = Vec<u8>> {
    data: Vec<T>,
    verify_minimaldata: bool,
}

#[cfg(test)]
impl<T> From<Vec<T>> for Stack<T> {
    fn from(data: Vec<T>) -> Self {
        Self {
            data,
            verify_minimaldata: true,
        }
    }
}

impl<T> Deref for Stack<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for Stack<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> Stack<T> {
    #[inline]
    pub fn new(verify_minimaldata: bool) -> Self {
        Self {
            data: Vec::new(),
            verify_minimaldata,
        }
    }

    // Ensure there are at least `n` elements on the stack.
    #[inline]
    pub fn require(&self, len: usize) -> Result<(), Error> {
        if self.data.len() < len {
            return Err(Error::InvalidStackOperation);
        }
        Ok(())
    }

    #[inline]
    pub fn last(&self) -> Result<&T, Error> {
        self.data.last().ok_or(Error::InvalidStackOperation)
    }

    #[inline]
    pub fn last_mut(&mut self) -> Result<&mut T, Error> {
        self.data.last_mut().ok_or(Error::InvalidStackOperation)
    }

    #[inline]
    pub fn pop(&mut self) -> Result<T, Error> {
        self.data.pop().ok_or(Error::InvalidStackOperation)
    }

    pub fn pop_num(&mut self) -> Result<ScriptNum, Error>
    where
        T: AsRef<[u8]>,
    {
        ScriptNum::from_bytes(self.pop()?.as_ref(), self.verify_minimaldata, None)
    }

    pub fn pop_num_with_max_size(&mut self, max_size: usize) -> Result<ScriptNum, Error>
    where
        T: AsRef<[u8]>,
    {
        ScriptNum::from_bytes(
            self.pop()?.as_ref(),
            self.verify_minimaldata,
            Some(max_size),
        )
    }

    /// Push an element onto the stack.
    #[inline]
    pub fn push(&mut self, value: T) {
        self.data.push(value)
    }

    #[inline]
    pub fn top(&self, i: usize) -> Result<&T, Error> {
        let pos = i + 1;
        self.require(pos)?;
        Ok(&self.data[self.data.len() - pos])
    }

    pub fn peek_bool(&self) -> Result<bool, Error>
    where
        T: AsRef<[u8]>,
    {
        Ok(cast_to_bool(self.last()?.as_ref()))
    }

    #[inline]
    pub fn remove(&mut self, i: usize) -> Result<T, Error> {
        let pos = i + 1;
        self.require(pos)?;
        let to_remove = self.data.len() - pos;
        Ok(self.data.remove(to_remove))
    }

    /// Removes the top `n` stack items.
    pub fn drop(&mut self, n: usize) -> Result<(), Error> {
        self.require(n)?;
        for _ in 0..n {
            self.data.pop();
        }
        Ok(())
    }

    /// Duplicates the top N items on the stack.
    ///
    /// dup(1): [x1 x2] -> [x1 x2 x2]
    /// dup(2): [x1 x2] -> [x1 x2 x1 x2]
    pub fn dup(&mut self, n: usize) -> Result<(), Error>
    where
        T: Clone,
    {
        self.require(n)?;
        let len = self.data.len();
        // Extend the stack with a clone of the top `n` items in place.
        self.data.extend_from_within(len - n..);
        Ok(())
    }

    /// Copies N items N items back to the top of the stack.
    ///
    /// over(1): [... x1 x2 x3] -> [... x1 x2 x3 x2]
    /// over(2): [... x1 x2 x3 x4] -> [... x1 x2 x3 x4 x1 x2]
    pub fn over(&mut self, n: usize) -> Result<(), Error>
    where
        T: Clone,
    {
        let count = n * 2;
        self.require(count)?;

        let len = self.data.len();

        // Extend the stack with a clone of the items `n` back in place.
        self.data.extend_from_within(len - count..len - count + n);

        Ok(())
    }

    /// Rotates the top 3N items on the stack to the left N times.
    ///
    /// - rot(1): [x1 x2 x3] -> [x2 x3 x1]
    /// - rot(2): [x1 x2 x3 x4 x5 x6] -> [x3 x4 x5 x6 x1 x2]
    pub fn rot(&mut self, n: usize) -> Result<(), Error>
    where
        T: Clone,
    {
        let count = n * 3;
        self.require(count)?;

        let len = self.data.len();
        let start_index = len - count;
        let slice = &mut self.data[start_index..];

        // Perform in-place rotation using swaps
        slice.rotate_left(n);

        Ok(())
    }

    // Swaps the top N items on the stack with those below them.
    //
    // Stack transformation:
    // - swap(1): [x1 x2] -> [x2 x1]
    // - swap(2): [x1 x2 x3 x4] -> [x3 x4 x1 x2]
    pub fn swap(&mut self, n: usize) -> Result<(), Error> {
        let count = n * 2;
        self.require(count)?;
        let len = self.data.len();
        // Use slices to perform the swap
        let (lower, upper) = self.data.split_at_mut(len - count + n);
        lower[len - count..].swap_with_slice(&mut upper[..n]);
        Ok(())
    }

    /// Removes the second-to-top stack item.
    ///
    /// nip: [x1 x2 x3] -> [x1 x3]
    pub fn nip(&mut self) -> Result<(), Error> {
        self.require(2)?;
        let len = self.data.len();
        self.data.swap_remove(len - 2);
        Ok(())
    }

    // Copies the item at the top of the stack and inserts it before the 2nd
    // to top item.
    //
    // [... x1 x2] -> [... x2 x1 x2]
    pub fn tuck(&mut self) -> Result<(), Error>
    where
        T: Clone,
    {
        self.require(2)?;
        let len = self.data.len();
        let v = self.data[len - 1].clone();
        self.data.insert(len - 2, v);
        Ok(())
    }
}

impl Stack<Vec<u8>> {
    #[inline]
    pub fn push_num(&mut self, num: ScriptNum) {
        self.data.push(num.to_bytes())
    }
}

fn cast_to_bool(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    if data[..data.len() - 1].iter().any(|x| x != &0) {
        return true;
    }

    let last = data[data.len() - 1];
    !(last == 0 || last == 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_require() {
        let stack: Stack<u8> = vec![].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Err(Error::InvalidStackOperation));
        let stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Ok(()));
        assert_eq!(stack.require(2), Err(Error::InvalidStackOperation));
        let stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Ok(()));
        assert_eq!(stack.require(2), Ok(()));
        assert_eq!(stack.require(3), Err(Error::InvalidStackOperation));
    }

    #[test]
    fn test_stack_last() {
        let stack: Stack<u8> = vec![].into();
        assert_eq!(stack.last(), Err(Error::InvalidStackOperation));
        let stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.last(), Ok(&0));
        let stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.last(), Ok(&5));
    }

    #[test]
    fn test_stack_pop() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.pop(), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.pop(), Ok(0));
        assert_eq!(stack, vec![].into());
        let mut stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.pop(), Ok(5));
        assert_eq!(stack.pop(), Ok(0));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_push() {
        let mut stack: Stack<u8> = vec![].into();
        stack.push(0);
        assert_eq!(stack, vec![0].into());
        stack.push(5);
        assert_eq!(stack, vec![0, 5].into());
    }

    #[test]
    fn test_stack_top() {
        let stack: Stack<u8> = vec![].into();
        assert_eq!(stack.top(0), Err(Error::InvalidStackOperation));
        let stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.top(0), Ok(&0));
        assert_eq!(stack.top(1), Err(Error::InvalidStackOperation));
        let stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.top(0), Ok(&5));
        assert_eq!(stack.top(1), Ok(&0));
        assert_eq!(stack.top(2), Err(Error::InvalidStackOperation));
    }

    #[test]
    fn test_stack_remove() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.remove(0), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.remove(0), Ok(0));
        assert_eq!(stack.remove(0), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.remove(1), Ok(0));
        assert_eq!(stack, vec![5].into());
        assert_eq!(stack.remove(0), Ok(5));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_drop() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.drop(0), Ok(()));
        let mut stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.drop(0), Ok(()));
        assert_eq!(stack, vec![0, 5].into());
        assert_eq!(stack.drop(3), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![0, 5].into());
        assert_eq!(stack.drop(1), Ok(()));
        assert_eq!(stack, vec![0].into());
        let mut stack: Stack<u8> = vec![3, 5, 0].into();
        assert_eq!(stack.drop(3), Ok(()));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_dup() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.dup(0), Ok(()));
        assert_eq!(stack.dup(1), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.dup(0), Ok(()));
        assert_eq!(stack.dup(2), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![0].into());
        assert_eq!(stack.dup(1), Ok(()));
        assert_eq!(stack, vec![0, 0].into());
        assert_eq!(stack.dup(2), Ok(()));
        assert_eq!(stack, vec![0, 0, 0, 0].into());
        let mut stack: Stack<u8> = vec![0, 1].into();
        assert_eq!(stack.dup(2), Ok(()));
        assert_eq!(stack, vec![0, 1, 0, 1].into());
    }

    #[test]
    fn test_stack_over() {
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.over(0), Ok(()));
        assert_eq!(stack.over(1), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.over(2), Err(Error::InvalidStackOperation));
        assert_eq!(stack.over(1), Ok(()));
        assert_eq!(stack, vec![0, 5, 0].into());
        assert_eq!(stack.over(1), Ok(()));
        assert_eq!(stack, vec![0, 5, 0, 5].into());
        let mut stack: Stack<u8> = vec![1, 2, 3, 4].into();
        assert_eq!(stack.over(2), Ok(()));
        assert_eq!(stack, vec![1, 2, 3, 4, 1, 2].into());
    }

    #[test]
    fn test_stack_rot() {
        let mut stack: Stack<u8> = vec![0, 5].into();
        assert_eq!(stack.rot(0), Ok(()));
        assert_eq!(stack.rot(1), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![0, 5].into());
        let mut stack: Stack<u8> = vec![0, 1, 2].into();
        assert_eq!(stack.rot(2), Err(Error::InvalidStackOperation));
        assert_eq!(stack.rot(1), Ok(()));
        assert_eq!(stack, vec![1, 2, 0].into());
        let mut stack: Stack<u8> = vec![0, 1, 2, 3].into();
        assert_eq!(stack.rot(1), Ok(()));
        assert_eq!(stack, vec![0, 2, 3, 1].into());
        let mut stack: Stack<u8> = vec![0, 1, 2, 3, 4, 5].into();
        assert_eq!(stack.rot(3), Err(Error::InvalidStackOperation));
        assert_eq!(stack.rot(2), Ok(()));
        assert_eq!(stack, vec![2, 3, 4, 5, 0, 1].into());
    }

    #[test]
    fn test_stack_swap() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.swap(0), Ok(()));
        assert_eq!(stack, vec![].into());
        assert_eq!(stack.swap(1), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0, 1, 2, 3].into();
        assert_eq!(stack.swap(0), Ok(()));
        assert_eq!(stack, vec![0, 1, 2, 3].into());
        assert_eq!(stack.swap(1), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 2].into());
        assert_eq!(stack.swap(2), Ok(()));
        assert_eq!(stack, vec![3, 2, 0, 1].into());
        assert_eq!(stack.swap(3), Err(Error::InvalidStackOperation));
        assert_eq!(stack, vec![3, 2, 0, 1].into());
    }

    #[test]
    fn test_stack_nip() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.nip(), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.nip(), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0, 1].into();
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![1].into());
        assert_eq!(stack.nip(), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0, 1, 2, 3].into();
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3].into());
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![0, 3].into());
    }

    #[test]
    fn test_stack_tuck() {
        let mut stack: Stack<u8> = vec![].into();
        assert_eq!(stack.tuck(), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0].into();
        assert_eq!(stack.tuck(), Err(Error::InvalidStackOperation));
        let mut stack: Stack<u8> = vec![0, 1].into();
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![1, 0, 1].into());
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![1, 1, 0, 1].into());
        let mut stack: Stack<u8> = vec![0, 1, 2, 3].into();
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 2, 3].into());
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 3, 2, 3].into());
    }
}
