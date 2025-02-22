use crate::num::ScriptNum;
use crate::VerifyFlags;
use std::fmt::Display;
use std::ops::{Deref, DerefMut};

/// Stack error type.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum StackError {
    #[error("invalid stack operation")]
    InvalidOperation,
    #[error(transparent)]
    Num(#[from] crate::num::NumError),
}

/// Stack for the script execution.
pub type Stack = GenericStack<Vec<u8>>;

impl Display for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stack {{ data: [")?;

        // Iterate over each Vec<u8> in `data` and display it as hex
        for (i, vec_item) in self.data.iter().enumerate() {
            if vec_item.is_empty() {
                write!(f, "00000000 <empty>")?;
            } else {
                for item in vec_item {
                    write!(f, "{:02x?}", item)?;
                }
            }

            // If this is not the last item, add a comma and a space
            if i != self.data.len() - 1 {
                write!(f, ", ")?;
            }
        }

        write!(f, "], verify_minimaldata: {} }}", self.verify_minimaldata)
    }
}

type Result<T> = std::result::Result<T, StackError>;

/// A stack used for managing script execution data with various operations.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct GenericStack<T = Vec<u8>> {
    data: Vec<T>,
    verify_minimaldata: bool,
}

#[cfg(test)]
impl<T> From<Vec<T>> for GenericStack<T> {
    fn from(data: Vec<T>) -> Self {
        Self {
            data,
            verify_minimaldata: false,
        }
    }
}

impl<T> Deref for GenericStack<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> DerefMut for GenericStack<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> GenericStack<T> {
    #[inline]
    pub fn new(data: Vec<T>, verify_minimaldata: bool) -> Self {
        Self {
            data,
            verify_minimaldata,
        }
    }

    pub fn with_data(data: Vec<T>) -> Self {
        Self {
            data,
            verify_minimaldata: false,
        }
    }

    pub fn with_flags(flags: &VerifyFlags) -> Self {
        Self {
            data: Vec::new(),
            verify_minimaldata: flags.verify_minimaldata(),
        }
    }

    /// Explicitly an empty stack, [].
    #[allow(unused)]
    pub fn empty() -> Self
    where
        T: Default,
    {
        Default::default()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    // Ensure there are at least `n` elements on the stack.
    #[inline]
    pub fn require(&self, len: usize) -> Result<()> {
        if self.data.len() < len {
            return Err(StackError::InvalidOperation);
        }
        Ok(())
    }

    /// Returns the last element of the stack.
    #[inline]
    pub fn last(&self) -> Result<&T> {
        self.data.last().ok_or(StackError::InvalidOperation)
    }

    /// Removes and returns the last element of the stack.
    #[inline]
    pub fn pop(&mut self) -> Result<T> {
        self.data.pop().ok_or(StackError::InvalidOperation)
    }

    /// Pops a number from the stack and converts it into a ScriptNum.
    #[inline]
    pub fn pop_num(&mut self) -> Result<ScriptNum>
    where
        T: AsRef<[u8]>,
    {
        ScriptNum::from_bytes(self.pop()?.as_ref(), self.verify_minimaldata, None)
            .map_err(Into::into)
    }

    /// Push an element onto the stack.
    #[inline]
    pub fn push(&mut self, value: T) -> &mut Self {
        self.data.push(value);
        self
    }

    /// Returns the element at the specified position from the top of the stack.
    ///
    /// `self.top(0)` is equalant to `self.last()`.
    #[inline]
    pub fn top(&self, i: usize) -> Result<&T> {
        let pos = i + 1;
        self.require(pos)?;
        Ok(&self.data[self.data.len() - pos])
    }

    /// Peeks the top element and converts it to a boolean.
    #[inline]
    pub fn peek_bool(&self) -> Result<bool>
    where
        T: AsRef<[u8]>,
    {
        Ok(cast_to_bool(self.last()?.as_ref()))
    }

    /// Pops the top element and converts it to a boolean.
    #[inline]
    pub fn pop_bool(&mut self) -> Result<bool>
    where
        T: AsRef<[u8]>,
    {
        Ok(cast_to_bool(self.pop()?.as_ref()))
    }

    /// Removes the element at the given index.
    #[inline]
    pub fn remove(&mut self, i: usize) -> Result<T> {
        let pos = i + 1;
        self.require(pos)?;
        let to_remove = self.data.len() - pos;
        Ok(self.data.remove(to_remove))
    }

    /// Removes the top `n` stack items.
    #[inline]
    pub fn drop(&mut self, n: usize) -> Result<()> {
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
    #[inline]
    pub fn dup(&mut self, n: usize) -> Result<()>
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
    #[inline]
    pub fn over(&mut self, n: usize) -> Result<()>
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
    #[inline]
    pub fn rot(&mut self, n: usize) -> Result<()>
    where
        T: Clone,
    {
        let count = n * 3;
        self.require(count)?;

        let len = self.data.len();
        let start_index = len - count;
        let slice = &mut self.data[start_index..];

        slice.rotate_left(n);

        Ok(())
    }

    // Swaps the top N items on the stack with those below them.
    //
    // GenericStack transformation:
    // - swap(1): [x1 x2] -> [x2 x1]
    // - swap(2): [x1 x2 x3 x4] -> [x3 x4 x1 x2]
    #[inline]
    pub fn swap(&mut self, n: usize) -> Result<()> {
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
    #[inline]
    pub fn nip(&mut self) -> Result<()> {
        self.require(2)?;
        let len = self.data.len();
        self.data.swap_remove(len - 2);
        Ok(())
    }

    // Copies the item at the top of the stack and inserts it before the 2nd
    // to top item.
    //
    // [... x1 x2] -> [... x2 x1 x2]
    #[inline]
    pub fn tuck(&mut self) -> Result<()>
    where
        T: Clone,
    {
        self.require(2)?;
        let len = self.data.len();
        let v = self.last().expect("Stack must be non-empty; qed").clone();
        self.data.insert(len - 2, v);
        Ok(())
    }
}

impl Stack {
    #[inline]
    pub fn push_num(&mut self, num: impl Into<ScriptNum>) -> &mut Self {
        self.push(num.into().as_bytes());
        self
    }

    #[inline]
    pub fn push_bool(&mut self, boolean: bool) -> &mut Self {
        if boolean {
            self.push(vec![1]);
        } else {
            self.push(Vec::new());
        }
        self
    }
}

/// Converts a byte slice to a boolean.
pub fn cast_to_bool(data: &[u8]) -> bool {
    match data.split_last() {
        Some((&last, rest)) => rest.iter().any(|&x| x != 0) || (last != 0 && last != 0x80),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestStack = GenericStack<u8>;

    #[test]
    fn test_stack_require() {
        let stack: TestStack = vec![].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Err(StackError::InvalidOperation));
        let stack: TestStack = vec![0].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Ok(()));
        assert_eq!(stack.require(2), Err(StackError::InvalidOperation));
        let stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.require(0), Ok(()));
        assert_eq!(stack.require(1), Ok(()));
        assert_eq!(stack.require(2), Ok(()));
        assert_eq!(stack.require(3), Err(StackError::InvalidOperation));
    }

    #[test]
    fn test_stack_last() {
        let stack: TestStack = vec![].into();
        assert_eq!(stack.last(), Err(StackError::InvalidOperation));
        let stack: TestStack = vec![0].into();
        assert_eq!(stack.last(), Ok(&0));
        let stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.last(), Ok(&5));
    }

    #[test]
    fn test_stack_pop() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.pop(), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.pop(), Ok(0));
        assert_eq!(stack, vec![].into());
        let mut stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.pop(), Ok(5));
        assert_eq!(stack.pop(), Ok(0));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_push() {
        let mut stack: TestStack = vec![].into();
        stack.push(0);
        assert_eq!(stack, vec![0].into());
        stack.push(5);
        assert_eq!(stack, vec![0, 5].into());
    }

    #[test]
    fn test_stack_top() {
        let stack: TestStack = vec![].into();
        assert_eq!(stack.top(0), Err(StackError::InvalidOperation));
        let stack: TestStack = vec![0].into();
        assert_eq!(stack.top(0), Ok(&0));
        assert_eq!(stack.top(1), Err(StackError::InvalidOperation));
        let stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.top(0), Ok(&5));
        assert_eq!(stack.top(1), Ok(&0));
        assert_eq!(stack.top(2), Err(StackError::InvalidOperation));
    }

    #[test]
    fn test_stack_remove() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.remove(0), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.remove(0), Ok(0));
        assert_eq!(stack.remove(0), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.remove(1), Ok(0));
        assert_eq!(stack, vec![5].into());
        assert_eq!(stack.remove(0), Ok(5));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_drop() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.drop(0), Ok(()));
        let mut stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.drop(0), Ok(()));
        assert_eq!(stack, vec![0, 5].into());
        assert_eq!(stack.drop(3), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![0, 5].into());
        assert_eq!(stack.drop(1), Ok(()));
        assert_eq!(stack, vec![0].into());
        let mut stack: TestStack = vec![3, 5, 0].into();
        assert_eq!(stack.drop(3), Ok(()));
        assert_eq!(stack, vec![].into());
    }

    #[test]
    fn test_stack_dup() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.dup(0), Ok(()));
        assert_eq!(stack.dup(1), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![].into());
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.dup(0), Ok(()));
        assert_eq!(stack.dup(2), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![0].into());
        assert_eq!(stack.dup(1), Ok(()));
        assert_eq!(stack, vec![0, 0].into());
        assert_eq!(stack.dup(2), Ok(()));
        assert_eq!(stack, vec![0, 0, 0, 0].into());
        let mut stack: TestStack = vec![0, 1].into();
        assert_eq!(stack.dup(2), Ok(()));
        assert_eq!(stack, vec![0, 1, 0, 1].into());
    }

    #[test]
    fn test_stack_over() {
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.over(0), Ok(()));
        assert_eq!(stack.over(1), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.over(2), Err(StackError::InvalidOperation));
        assert_eq!(stack.over(1), Ok(()));
        assert_eq!(stack, vec![0, 5, 0].into());
        assert_eq!(stack.over(1), Ok(()));
        assert_eq!(stack, vec![0, 5, 0, 5].into());
        let mut stack: TestStack = vec![1, 2, 3, 4].into();
        assert_eq!(stack.over(2), Ok(()));
        assert_eq!(stack, vec![1, 2, 3, 4, 1, 2].into());
    }

    #[test]
    fn test_stack_rot() {
        let mut stack: TestStack = vec![0, 5].into();
        assert_eq!(stack.rot(0), Ok(()));
        assert_eq!(stack.rot(1), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![0, 5].into());
        let mut stack: TestStack = vec![0, 1, 2].into();
        assert_eq!(stack.rot(2), Err(StackError::InvalidOperation));
        assert_eq!(stack.rot(1), Ok(()));
        assert_eq!(stack, vec![1, 2, 0].into());
        let mut stack: TestStack = vec![0, 1, 2, 3].into();
        assert_eq!(stack.rot(1), Ok(()));
        assert_eq!(stack, vec![0, 2, 3, 1].into());
        let mut stack: TestStack = vec![0, 1, 2, 3, 4, 5].into();
        assert_eq!(stack.rot(3), Err(StackError::InvalidOperation));
        assert_eq!(stack.rot(2), Ok(()));
        assert_eq!(stack, vec![2, 3, 4, 5, 0, 1].into());
    }

    #[test]
    fn test_stack_swap() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.swap(0), Ok(()));
        assert_eq!(stack, vec![].into());
        assert_eq!(stack.swap(1), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0, 1, 2, 3].into();
        assert_eq!(stack.swap(0), Ok(()));
        assert_eq!(stack, vec![0, 1, 2, 3].into());
        assert_eq!(stack.swap(1), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 2].into());
        assert_eq!(stack.swap(2), Ok(()));
        assert_eq!(stack, vec![3, 2, 0, 1].into());
        assert_eq!(stack.swap(3), Err(StackError::InvalidOperation));
        assert_eq!(stack, vec![3, 2, 0, 1].into());
    }

    #[test]
    fn test_stack_nip() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.nip(), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.nip(), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0, 1].into();
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![1].into());
        assert_eq!(stack.nip(), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0, 1, 2, 3].into();
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3].into());
        assert_eq!(stack.nip(), Ok(()));
        assert_eq!(stack, vec![0, 3].into());
    }

    #[test]
    fn test_stack_tuck() {
        let mut stack: TestStack = vec![].into();
        assert_eq!(stack.tuck(), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0].into();
        assert_eq!(stack.tuck(), Err(StackError::InvalidOperation));
        let mut stack: TestStack = vec![0, 1].into();
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![1, 0, 1].into());
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![1, 1, 0, 1].into());
        let mut stack: TestStack = vec![0, 1, 2, 3].into();
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 2, 3].into());
        assert_eq!(stack.tuck(), Ok(()));
        assert_eq!(stack, vec![0, 1, 3, 3, 2, 3].into());
    }
}
