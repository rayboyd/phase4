//! The libxpc C API the service uses, declared by hand, with owned and
//! borrowed wrappers that keep reference counting and type checks in one
//! place.
//!
//! Functions that create objects return a retained reference, which `Owned`
//! releases on drop. `xpc_dictionary_get_value` and `xpc_array_get_value`
//! return borrowed references, which the `DictionaryRef` and `ArrayRef` views
//! never release.

#![allow(non_camel_case_types, non_upper_case_globals)]

use block2::{Block, RcBlock};
use std::ffi::{c_char, c_void, CStr, CString};
use std::marker::PhantomData;
use std::ptr::{self, addr_of, NonNull};

pub(crate) type xpc_object_t = *mut c_void;
pub(crate) type xpc_connection_t = xpc_object_t;
pub(crate) type xpc_type_t = *const c_void;
pub(crate) type xpc_handler_t = Block<dyn Fn(xpc_object_t)>;

/// The index `xpc_array_set_int64` treats as append.
pub(crate) const XPC_ARRAY_APPEND: usize = usize::MAX;

/// The character that replaces an interior NUL byte in a string sent over XPC.
const NUL_REPLACEMENT: &str = "\u{FFFD}";

extern "C" {
    pub(crate) static _xpc_type_dictionary: u8;
    pub(crate) static _xpc_type_array: u8;
    pub(crate) static _xpc_type_string: u8;
    pub(crate) static _xpc_type_int64: u8;
    pub(crate) static _xpc_type_double: u8;
    pub(crate) static _xpc_type_bool: u8;
    pub(crate) static _xpc_type_shmem: u8;
    pub(crate) static _xpc_type_error: u8;
    pub(crate) static _xpc_type_connection: u8;
    pub(crate) static _xpc_error_connection_invalid: u8;
    pub(crate) static _xpc_error_connection_interrupted: u8;
    pub(crate) static _xpc_error_termination_imminent: u8;

    pub(crate) fn xpc_main(handler: extern "C" fn(xpc_connection_t)) -> !;
    #[cfg(test)]
    pub(crate) fn xpc_connection_create(
        name: *const c_char,
        targetq: *mut c_void,
    ) -> xpc_connection_t;
    #[cfg(test)]
    pub(crate) fn xpc_connection_create_from_endpoint(endpoint: xpc_object_t) -> xpc_connection_t;
    #[cfg(test)]
    pub(crate) fn xpc_endpoint_create(connection: xpc_connection_t) -> xpc_object_t;
    pub(crate) fn xpc_connection_set_event_handler(
        connection: xpc_connection_t,
        handler: &xpc_handler_t,
    );
    pub(crate) fn xpc_connection_resume(connection: xpc_connection_t);
    pub(crate) fn xpc_connection_cancel(connection: xpc_connection_t);
    pub(crate) fn xpc_connection_send_message(connection: xpc_connection_t, message: xpc_object_t);
    #[cfg(test)]
    pub(crate) fn xpc_connection_send_message_with_reply_sync(
        connection: xpc_connection_t,
        message: xpc_object_t,
    ) -> xpc_object_t;
    pub(crate) fn xpc_get_type(object: xpc_object_t) -> xpc_type_t;
    pub(crate) fn xpc_retain(object: xpc_object_t) -> xpc_object_t;
    pub(crate) fn xpc_release(object: xpc_object_t);
    pub(crate) fn xpc_dictionary_create(
        keys: *const *const c_char,
        values: *const xpc_object_t,
        count: usize,
    ) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_create_reply(original: xpc_object_t) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_get_value(
        dictionary: xpc_object_t,
        key: *const c_char,
    ) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_set_value(
        dictionary: xpc_object_t,
        key: *const c_char,
        value: xpc_object_t,
    );
    pub(crate) fn xpc_dictionary_set_int64(
        dictionary: xpc_object_t,
        key: *const c_char,
        value: i64,
    );
    #[cfg(test)]
    pub(crate) fn xpc_dictionary_set_double(
        dictionary: xpc_object_t,
        key: *const c_char,
        value: f64,
    );
    pub(crate) fn xpc_dictionary_set_bool(
        dictionary: xpc_object_t,
        key: *const c_char,
        value: bool,
    );
    pub(crate) fn xpc_dictionary_set_string(
        dictionary: xpc_object_t,
        key: *const c_char,
        string: *const c_char,
    );
    pub(crate) fn xpc_int64_get_value(object: xpc_object_t) -> i64;
    pub(crate) fn xpc_double_get_value(object: xpc_object_t) -> f64;
    #[cfg(test)]
    pub(crate) fn xpc_bool_get_value(object: xpc_object_t) -> bool;
    pub(crate) fn xpc_string_get_string_ptr(object: xpc_object_t) -> *const c_char;
    pub(crate) fn xpc_array_create(objects: *const xpc_object_t, count: usize) -> xpc_object_t;
    pub(crate) fn xpc_array_append_value(array: xpc_object_t, value: xpc_object_t);
    pub(crate) fn xpc_array_set_int64(array: xpc_object_t, index: usize, value: i64);
    pub(crate) fn xpc_array_get_count(array: xpc_object_t) -> usize;
    pub(crate) fn xpc_array_get_value(array: xpc_object_t, index: usize) -> xpc_object_t;
    pub(crate) fn xpc_shmem_create(region: *mut c_void, length: usize) -> xpc_object_t;
    #[cfg(test)]
    pub(crate) fn xpc_shmem_map(xshmem: xpc_object_t, region: *mut *mut c_void) -> usize;
    pub(crate) fn xpc_transaction_begin();
    pub(crate) fn xpc_transaction_end();
}

/// The XPC types the service distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Dictionary,
    Array,
    String,
    Int64,
    Double,
    Bool,
    Shmem,
    Error,
    Connection,
    Other,
}

/// The type of `object`, compared by address against the `_xpc_type_*` symbols.
pub(crate) fn kind(object: xpc_object_t) -> Kind {
    if object.is_null() {
        return Kind::Other;
    }
    // SAFETY: `object` is a live XPC object handed to us by libxpc, and the
    // type symbols are only compared by address.
    unsafe {
        let object_type = xpc_get_type(object);
        let types: [(*const u8, Kind); 9] = [
            (addr_of!(_xpc_type_dictionary), Kind::Dictionary),
            (addr_of!(_xpc_type_array), Kind::Array),
            (addr_of!(_xpc_type_string), Kind::String),
            (addr_of!(_xpc_type_int64), Kind::Int64),
            (addr_of!(_xpc_type_double), Kind::Double),
            (addr_of!(_xpc_type_bool), Kind::Bool),
            (addr_of!(_xpc_type_shmem), Kind::Shmem),
            (addr_of!(_xpc_type_error), Kind::Error),
            (addr_of!(_xpc_type_connection), Kind::Connection),
        ];
        types
            .iter()
            .find(|(symbol, _)| ptr::eq(object_type, symbol.cast()))
            .map_or(Kind::Other, |(_, found)| *found)
    }
}

/// The connection errors the service reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionError {
    Invalid,
    Interrupted,
    TerminationImminent,
    Other,
}

/// The connection error `event` is, or `None` when it is not an error.
pub(crate) fn connection_error(event: xpc_object_t) -> Option<ConnectionError> {
    if kind(event) != Kind::Error {
        return None;
    }
    // The error symbols are global objects compared by address only.
    let error = if ptr::eq(
        event.cast_const(),
        addr_of!(_xpc_error_connection_invalid).cast(),
    ) {
        ConnectionError::Invalid
    } else if ptr::eq(
        event.cast_const(),
        addr_of!(_xpc_error_connection_interrupted).cast(),
    ) {
        ConnectionError::Interrupted
    } else if ptr::eq(
        event.cast_const(),
        addr_of!(_xpc_error_termination_imminent).cast(),
    ) {
        ConnectionError::TerminationImminent
    } else {
        ConnectionError::Other
    };
    Some(error)
}

/// An owned XPC object reference, released on drop.
#[derive(Debug)]
pub(crate) struct Owned(NonNull<c_void>);

// SAFETY: XPC objects are reference counted and thread safe to retain,
// release and send.
unsafe impl Send for Owned {}
// SAFETY: as above, every method only reads through libxpc's own locking.
unsafe impl Sync for Owned {}

impl Owned {
    /// Takes ownership of a retained reference. `None` for a null pointer.
    ///
    /// # Safety
    ///
    /// `object` must be null or a retained XPC object the caller owns.
    pub(crate) unsafe fn from_retained(object: xpc_object_t) -> Option<Self> {
        NonNull::new(object).map(Self)
    }

    /// Retains a borrowed reference. `None` for a null pointer.
    ///
    /// # Safety
    ///
    /// `object` must be null or a live XPC object.
    pub(crate) unsafe fn retain(object: xpc_object_t) -> Option<Self> {
        let object = NonNull::new(object)?;
        xpc_retain(object.as_ptr());
        Some(Self(object))
    }

    pub(crate) fn as_ptr(&self) -> xpc_object_t {
        self.0.as_ptr()
    }

    #[cfg(test)]
    /// A typed view when this object is a dictionary.
    pub(crate) fn as_dictionary(&self) -> Option<DictionaryRef<'_>> {
        // SAFETY: the view borrows `self`, which keeps the object alive.
        unsafe { DictionaryRef::from_ptr(self.as_ptr()) }
    }

    #[cfg(test)]
    /// The type of this object.
    pub(crate) fn kind(&self) -> Kind {
        kind(self.as_ptr())
    }
}

impl Clone for Owned {
    fn clone(&self) -> Self {
        // SAFETY: `self` holds a live reference.
        unsafe { xpc_retain(self.as_ptr()) };
        Self(self.0)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `self` owns exactly one reference.
        unsafe { xpc_release(self.as_ptr()) };
    }
}

/// A borrowed dictionary with type checked reads. Each getter returns `None`
/// when the key is missing or holds another XPC type.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DictionaryRef<'a> {
    pointer: xpc_object_t,
    _owner: PhantomData<&'a Owned>,
}

impl<'a> DictionaryRef<'a> {
    /// A view of `object` when it is a dictionary.
    ///
    /// # Safety
    ///
    /// `object` must be null or a live XPC object that outlives `'a`.
    pub(crate) unsafe fn from_ptr(object: xpc_object_t) -> Option<Self> {
        (kind(object) == Kind::Dictionary).then_some(Self {
            pointer: object,
            _owner: PhantomData,
        })
    }

    pub(crate) fn as_ptr(self) -> xpc_object_t {
        self.pointer
    }

    pub(crate) fn get_value(self, key: &CStr) -> Option<xpc_object_t> {
        // SAFETY: the dictionary is live for `'a`, and `key` is NUL terminated.
        let value = unsafe { xpc_dictionary_get_value(self.pointer, key.as_ptr()) };
        (!value.is_null()).then_some(value)
    }

    pub(crate) fn contains(self, key: &CStr) -> bool {
        self.get_value(key).is_some()
    }

    pub(crate) fn get_i64(self, key: &CStr) -> Option<i64> {
        let value = self
            .get_value(key)
            .filter(|value| kind(*value) == Kind::Int64)?;
        // SAFETY: `value` is a live int64 object.
        Some(unsafe { xpc_int64_get_value(value) })
    }

    pub(crate) fn get_f64(self, key: &CStr) -> Option<f64> {
        let value = self
            .get_value(key)
            .filter(|value| kind(*value) == Kind::Double)?;
        // SAFETY: `value` is a live double object.
        Some(unsafe { xpc_double_get_value(value) })
    }

    #[cfg(test)]
    pub(crate) fn get_bool(self, key: &CStr) -> Option<bool> {
        let value = self
            .get_value(key)
            .filter(|value| kind(*value) == Kind::Bool)?;
        // SAFETY: `value` is a live bool object.
        Some(unsafe { xpc_bool_get_value(value) })
    }

    /// `None` also when the string is not valid UTF-8.
    pub(crate) fn get_string(self, key: &CStr) -> Option<String> {
        let value = self
            .get_value(key)
            .filter(|value| kind(*value) == Kind::String)?;
        // SAFETY: `value` is a live string object, whose storage is NUL
        // terminated and lives as long as the object.
        let text = unsafe { CStr::from_ptr(xpc_string_get_string_ptr(value)) };
        text.to_str().ok().map(str::to_owned)
    }

    pub(crate) fn get_dictionary(self, key: &CStr) -> Option<DictionaryRef<'a>> {
        // SAFETY: the value belongs to this dictionary, which lives for `'a`.
        unsafe { DictionaryRef::from_ptr(self.get_value(key)?) }
    }

    pub(crate) fn get_array(self, key: &CStr) -> Option<ArrayRef<'a>> {
        // SAFETY: the value belongs to this dictionary, which lives for `'a`.
        unsafe { ArrayRef::from_ptr(self.get_value(key)?) }
    }
}

/// A borrowed array with type checked reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArrayRef<'a> {
    pointer: xpc_object_t,
    _owner: PhantomData<&'a Owned>,
}

impl ArrayRef<'_> {
    /// A view of `object` when it is an array.
    ///
    /// # Safety
    ///
    /// `object` must be null or a live XPC object that outlives the view.
    pub(crate) unsafe fn from_ptr(object: xpc_object_t) -> Option<Self> {
        (kind(object) == Kind::Array).then_some(Self {
            pointer: object,
            _owner: PhantomData,
        })
    }

    pub(crate) fn len(self) -> usize {
        // SAFETY: the array outlives the view.
        unsafe { xpc_array_get_count(self.pointer) }
    }

    fn get_value(self, index: usize) -> Option<xpc_object_t> {
        if index >= self.len() {
            return None;
        }
        // SAFETY: the index is within the array, which outlives the view.
        let value = unsafe { xpc_array_get_value(self.pointer, index) };
        (!value.is_null()).then_some(value)
    }

    pub(crate) fn get_i64(self, index: usize) -> Option<i64> {
        let value = self
            .get_value(index)
            .filter(|value| kind(*value) == Kind::Int64)?;
        // SAFETY: `value` is a live int64 object.
        Some(unsafe { xpc_int64_get_value(value) })
    }
}

#[cfg(test)]
impl<'a> ArrayRef<'a> {
    pub(crate) fn get_dictionary(self, index: usize) -> Option<DictionaryRef<'a>> {
        // SAFETY: the value belongs to this array, which lives for `'a`.
        unsafe { DictionaryRef::from_ptr(self.get_value(index)?) }
    }
}

/// A C string for a value sent over XPC, with interior NUL bytes replaced.
fn c_string(value: &str) -> CString {
    CString::new(value.replace('\0', NUL_REPLACEMENT)).expect("interior NUL bytes were replaced")
}

/// A dictionary under construction, created empty or as a reply.
pub(crate) struct DictionaryBuilder(Owned);

impl DictionaryBuilder {
    pub(crate) fn new() -> Self {
        // SAFETY: an empty dictionary with no keys or values.
        let dictionary = unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) };
        // SAFETY: xpc_dictionary_create returns a retained object.
        Self(unsafe { Owned::from_retained(dictionary) }.expect("libxpc creates a dictionary"))
    }

    /// Creates a reply to `message`. `None` when `message` expects no reply.
    pub(crate) fn reply_to(message: DictionaryRef<'_>) -> Option<Self> {
        // SAFETY: `message` is a live dictionary.
        let reply = unsafe { xpc_dictionary_create_reply(message.as_ptr()) };
        // SAFETY: xpc_dictionary_create_reply returns a retained object or null.
        unsafe { Owned::from_retained(reply) }.map(Self)
    }

    pub(crate) fn set_i64(&mut self, key: &CStr, value: i64) -> &mut Self {
        // SAFETY: the dictionary is owned and `key` is NUL terminated.
        unsafe { xpc_dictionary_set_int64(self.0.as_ptr(), key.as_ptr(), value) };
        self
    }

    #[cfg(test)]
    pub(crate) fn set_f64(&mut self, key: &CStr, value: f64) -> &mut Self {
        // SAFETY: the dictionary is owned and `key` is NUL terminated.
        unsafe { xpc_dictionary_set_double(self.0.as_ptr(), key.as_ptr(), value) };
        self
    }

    pub(crate) fn set_bool(&mut self, key: &CStr, value: bool) -> &mut Self {
        // SAFETY: the dictionary is owned and `key` is NUL terminated.
        unsafe { xpc_dictionary_set_bool(self.0.as_ptr(), key.as_ptr(), value) };
        self
    }

    /// Interior NUL bytes are replaced with U+FFFD.
    pub(crate) fn set_str(&mut self, key: &CStr, value: &str) -> &mut Self {
        let value = c_string(value);
        // SAFETY: the dictionary is owned, and libxpc copies the string.
        unsafe { xpc_dictionary_set_string(self.0.as_ptr(), key.as_ptr(), value.as_ptr()) };
        self
    }

    pub(crate) fn set_value(&mut self, key: &CStr, value: &Owned) -> &mut Self {
        // SAFETY: both objects are live, and the dictionary retains `value`.
        unsafe { xpc_dictionary_set_value(self.0.as_ptr(), key.as_ptr(), value.as_ptr()) };
        self
    }

    pub(crate) fn into_owned(self) -> Owned {
        self.0
    }
}

/// An array under construction.
pub(crate) struct ArrayBuilder(Owned);

impl ArrayBuilder {
    pub(crate) fn new() -> Self {
        // SAFETY: an empty array with no values.
        let array = unsafe { xpc_array_create(ptr::null(), 0) };
        // SAFETY: xpc_array_create returns a retained object.
        Self(unsafe { Owned::from_retained(array) }.expect("libxpc creates an array"))
    }

    pub(crate) fn push_i64(&mut self, value: i64) -> &mut Self {
        // SAFETY: the array is owned, and XPC_ARRAY_APPEND appends.
        unsafe { xpc_array_set_int64(self.0.as_ptr(), XPC_ARRAY_APPEND, value) };
        self
    }

    pub(crate) fn push(&mut self, value: &Owned) -> &mut Self {
        // SAFETY: both objects are live, and the array retains `value`.
        unsafe { xpc_array_append_value(self.0.as_ptr(), value.as_ptr()) };
        self
    }

    pub(crate) fn into_owned(self) -> Owned {
        self.0
    }
}

/// Sends `message` on `connection`.
pub(crate) fn send(connection: &Owned, message: &Owned) {
    // SAFETY: both objects are live. libxpc retains the message while sending.
    unsafe { xpc_connection_send_message(connection.as_ptr(), message.as_ptr()) };
}

#[cfg(test)]
/// Sends `message` and waits for its reply, which is a dictionary or an
/// error object.
pub(crate) fn send_with_reply_sync(connection: &Owned, message: &Owned) -> Option<Owned> {
    // SAFETY: both objects are live, and the reply is returned retained.
    unsafe {
        Owned::from_retained(xpc_connection_send_message_with_reply_sync(
            connection.as_ptr(),
            message.as_ptr(),
        ))
    }
}

/// Sets `handler` as the connection's event handler, then resumes it.
pub(crate) fn activate(connection: &Owned, handler: impl Fn(xpc_object_t) + Send + Sync + 'static) {
    let block = RcBlock::new(move |event: xpc_object_t| handler(event));
    // SAFETY: the connection is live, libxpc copies the block, and resuming
    // an inactive connection once is required before it delivers events.
    unsafe {
        xpc_connection_set_event_handler(connection.as_ptr(), &block);
        xpc_connection_resume(connection.as_ptr());
    }
}

/// Cancels `connection`. Its handler receives `XPC_ERROR_CONNECTION_INVALID`.
pub(crate) fn cancel(connection: &Owned) {
    // SAFETY: the connection is live.
    unsafe { xpc_connection_cancel(connection.as_ptr()) };
}

/// An anonymous listener connection, which peers reach through its endpoint.
#[cfg(test)]
pub(crate) fn anonymous_listener() -> Owned {
    // SAFETY: a null name and queue create an anonymous listener.
    let listener = unsafe { xpc_connection_create(ptr::null(), ptr::null_mut()) };
    // SAFETY: xpc_connection_create returns a retained connection.
    unsafe { Owned::from_retained(listener) }.expect("libxpc creates an anonymous listener")
}

/// An endpoint for `listener`.
#[cfg(test)]
pub(crate) fn endpoint(listener: &Owned) -> Owned {
    // SAFETY: the listener is live, and the endpoint is returned retained.
    unsafe { Owned::from_retained(xpc_endpoint_create(listener.as_ptr())) }
        .expect("libxpc creates an endpoint")
}

/// A new, inactive client connection to `endpoint`.
#[cfg(test)]
pub(crate) fn connect(endpoint: &Owned) -> Owned {
    // SAFETY: the endpoint is live, and the connection is returned retained.
    unsafe { Owned::from_retained(xpc_connection_create_from_endpoint(endpoint.as_ptr())) }
        .expect("libxpc creates a connection from an endpoint")
}

/// Boxes a `MAP_SHARED` region for sending.
///
/// # Safety
///
/// `region` must be the start of a `mmap` mapping with `MAP_SHARED` of at
/// least `length` bytes.
pub(crate) unsafe fn shmem_create(region: *mut c_void, length: usize) -> Option<Owned> {
    Owned::from_retained(xpc_shmem_create(region, length))
}

/// Maps a shared memory object. Returns the mapping and its length, which
/// the caller unmaps with `munmap`.
#[cfg(test)]
pub(crate) fn shmem_map(object: xpc_object_t) -> Option<(NonNull<c_void>, usize)> {
    if kind(object) != Kind::Shmem {
        return None;
    }
    let mut region = ptr::null_mut();
    // SAFETY: `object` is a live shared memory object.
    let length = unsafe { xpc_shmem_map(object, &raw mut region) };
    NonNull::new(region)
        .filter(|_| length > 0)
        .map(|region| (region, length))
}

/// Tells the runtime the service is busy, so it does not exit while idle.
pub(crate) fn transaction_begin() {
    // SAFETY: a counter in the XPC runtime with no arguments.
    unsafe { xpc_transaction_begin() };
}

/// Ends a transaction `transaction_begin` started.
pub(crate) fn transaction_end() {
    // SAFETY: called once for each `transaction_begin`.
    unsafe { xpc_transaction_end() };
}
