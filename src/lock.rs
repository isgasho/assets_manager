//! Definitions of cache entries and locks

use std::{
    any::Any,
    fmt,
    hash,
    ops::Deref,
};


#[cfg(feature = "parking_lot")]
use parking_lot as sync;
#[cfg(not(feature = "parking_lot"))]
use std::sync;

pub(crate) use sync::{RwLockReadGuard, RwLockWriteGuard};


#[cfg(feature = "parking_lot")]
#[inline]
fn wrap<T>(param: T) -> T {
    param
}

#[cfg(not(feature = "parking_lot"))]
#[inline]
fn wrap<T>(param: sync::LockResult<T>) -> T {
    param.unwrap_or_else(sync::PoisonError::into_inner)
}


/// `RwLock` from `parking_lot` and `std` have different APIs, so we use this
/// simple wrapper to easily permit both.
pub(crate) struct RwLock<T: ?Sized>(sync::RwLock<T>);

impl<T> RwLock<T> {
    #[inline]
    pub fn new(inner: T) -> Self {
        Self(sync::RwLock::new(inner))
    }

    #[inline]
    pub fn into_inner(self) -> T {
        wrap(self.0.into_inner())
    }
}

impl<T: ?Sized> RwLock<T> {
    #[inline]
    pub fn read(&self) -> RwLockReadGuard<T> {
        wrap(self.0.read())
    }

    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<T> {
        wrap(self.0.write())
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        wrap(self.0.get_mut())
    }
}


#[cfg(feature = "hot-reloading")]
pub(crate) struct Mutex<T: ?Sized>(sync::Mutex<T>);

#[cfg(feature = "hot-reloading")]
impl<T> Mutex<T> {
    #[inline]
    pub fn new(inner: T) -> Self {
        Self(sync::Mutex::new(inner))
    }
}

#[cfg(feature = "hot-reloading")]
impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub fn lock(&self) -> sync::MutexGuard<T> {
        wrap(self.0.lock())
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        wrap(self.0.get_mut())
    }
}


/// An entry in the cache
///
/// # Safety
///
/// - Methods that are generic over `T` can only be called with the same `T` used
/// to create them.
/// - When an `AssetRef<'a, T>` is returned, you have to ensure that `self`
/// outlives it. The `CacheEntry` can be moved but cannot be dropped.
///
/// [`ContreteCacheEntry`]: struct.ContreteCacheEntry.html
pub(crate) struct CacheEntry(Box<dyn Any + Send + Sync>);

impl<'a> CacheEntry {
    /// Creates a new `CacheEntry` containing an asset of type `T`.
    ///
    /// The returned structure can safely use its methods with type parameter `T`.
    #[inline]
    pub fn new<T: Send + Sync + 'static>(asset: T) -> Self {
        CacheEntry(Box::new(RwLock::new(asset)))
    }

    /// Returns a reference to the underlying lock.
    ///
    /// # Safety
    ///
    /// See type-level documentation.
    #[inline]
    pub unsafe fn get_ref<T: Send + Sync + 'static>(&self) -> AssetRef<'a, T> {
        debug_assert!(self.0.is::<RwLock<T>>());

        let data = {
            let ptr = &*self.0 as *const dyn Any as *const RwLock<T>;
            &*ptr
        };

        AssetRef { data }
    }

    /// Write a value and a get reference to the underlying lock
    ///
    /// # Safety
    ///
    /// See type-level documentation.
    pub unsafe fn write<T: Send + Sync + 'static>(&self, asset: T) -> AssetRef<'a, T> {
        let lock = self.get_ref();
        let mut cached_guard = lock.data.write();
        *cached_guard = asset;
        drop(cached_guard);
        lock
    }

    /// Consumes the `CacheEntry` and returns its inner value.
    ///
    /// # Safety
    ///
    /// See type-level documentation.
    #[inline]
    pub unsafe fn into_inner<T: Send + Sync + 'static>(self) -> T {
        debug_assert!(self.0.is::<RwLock<T>>());

        Box::from_raw(Box::into_raw(self.0) as *mut RwLock<T>).into_inner()
    }
}

impl fmt::Debug for CacheEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("CacheEntry")
    }
}


/// A lock on an asset.
///
/// The type parameter `A` represents type of the locked asset.
///
/// This structure wraps a RwLock, so assets can be written to be reloaded. As
/// such, any number of read guard can exist at the same time, but none can
/// exist while reloading an asset.
///
/// This is the structure you want to use to store a reference to an asset.
/// However, shared data threads is usually required to be `'static`. The first
/// solution is to create static `AssetCache`s and references (for example with
/// `lazy_static` crate). You can also use crates allow threads with non-static
/// data (such as `crossbeam-utils::scope`).
pub struct AssetRef<'a, A> {
    data: &'a RwLock<A>,
}

impl<'a, A> AssetRef<'a, A> {
    /// Locks the pointed asset for reading.
    ///
    /// Returns a RAII guard which will release the lock once dropped.
    #[inline]
    pub fn read(&self) -> AssetGuard<'a, A> {
        AssetGuard {
            guard: self.data.read(),
        }
    }

    /// Checks if the two assets refer to the same cache entry
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.data, other.data)
    }
}

impl<A> AssetRef<'_, A>
where
    A: Clone
{
    /// Returns a cloned version of the inner asset.
    #[inline]
    pub fn cloned(self) -> A {
        self.data.read().clone()
    }
}

impl<A> Clone for AssetRef<'_, A> {
    fn clone(&self) -> Self {
        Self {
            data: self.data,
        }
    }
}

impl<A> Copy for AssetRef<'_, A> {}

impl<A> hash::Hash for AssetRef<'_, A>
where
    A: hash::Hash,
{
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.data.read().hash(state);
    }
}

impl<A> fmt::Debug for AssetRef<'_, A>
where
    A: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetRef").field("data", &*self.data.read()).finish()
    }
}

/// RAII guard used to keep a read lock on an asset and release it when dropped.
///
/// It can be obtained by calling [`AssetRef::read`].
///
/// [`AssetRef::read`]: struct.AssetRef.html#method.read
pub struct AssetGuard<'a, A> {
    guard: RwLockReadGuard<'a, A>,
}

impl<A> Deref for AssetGuard<'_, A> {
    type Target = A;

    #[inline]
    fn deref(&self) -> &A {
        &self.guard
    }
}

impl<A> fmt::Display for AssetGuard<'_, A>
where
    A: fmt::Display,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<A> fmt::Debug for AssetGuard<'_, A>
where
    A: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
