from setuptools import setup
from setuptools_rust import Binding, RustExtension

setup(
    name="rmcp-python",
    version="0.1.0",
    rust_extensions=[RustExtension("rmcp_python.rmcp_python", "Cargo.toml", binding=Binding.PyO3)],
    packages=["rmcp_python"],
    # Rust extension is not zip safe
    zip_safe=False,
)