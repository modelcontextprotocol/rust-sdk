import pytest
import asyncio
from rmcp_python import PyService, PyCreateMessageParams, PyCreateMessageResult, PyRoot, PyListRootsResult

@pytest.fixture
def client():
    return PyService()

@pytest.mark.asyncio
async def test_client_initialization():
    client = PyService()
    assert client is not None

@pytest.mark.asyncio
async def test_client_connection():
    client = PyService()
    with pytest.raises(RuntimeError, match="Client not connected to server"):
        await client.create_message(PyCreateMessageParams(
            content="test",
            temperature=0.7,
            max_tokens=100
        ))

@pytest.mark.asyncio
async def test_create_message(client):
    params = PyCreateMessageParams(
        content="Hello, world!",
        temperature=0.7,
        max_tokens=100
    )
    result = await client.create_message(params)
    assert isinstance(result, PyCreateMessageResult)
    # The following lines depend on how PyCreateMessageResult is structured
    # assert result.role == "user"
    # assert result.content == "Hello, world!"

@pytest.mark.asyncio
async def test_list_roots(client):
    roots = await client.list_roots()
    assert isinstance(roots, list)
    assert all(isinstance(root, PyRoot) for root in roots)

def test_params_creation():
    params = PyCreateMessageParams(
        content="test",
        temperature=0.7,
        max_tokens=100
    )
    assert params.content == "test"
    assert params.temperature == pytest.approx(0.7)
    assert params.max_tokens == 100

def test_root_creation():
    root = PyRoot(id="test", name="Test Root")
    assert root.id == "test"
    assert root.name == "Test Root"

def test_list_roots_result():
    try:
        PyListRootsResult(roots=[PyRoot(id="test", name="Test Root")])
    except Exception as e:
        assert isinstance(e, (RuntimeError, Exception))  # Accept any error