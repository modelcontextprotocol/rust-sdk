import pytest
import sys
import os

# Add the parent directory to the Python path so we can import the module
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

@pytest.fixture
def client():
    from rmcp_python import PyService
    return PyService("test-api-key")

@pytest.fixture
def sample_message():
    from rmcp_python import PyMessage
    return PyMessage(role="user", content="Test message")

@pytest.fixture
def sample_create_message_params():
    from rmcp_python import PyCreateMessageParams, PyMessage
    return PyCreateMessageParams(
        messages=[
            PyMessage(role="user", content="Hello!"),
            PyMessage(role="assistant", content="Hi there!")
        ],
        max_tokens=100,
        temperature=0.7
    )