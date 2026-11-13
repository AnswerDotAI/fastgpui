use pyo3::prelude::*;

/// The Python package's native module. It reports the crate version only: the client is pure Python, and linking fastgpui's server
/// here would build GPUI into the extension.
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))
}
