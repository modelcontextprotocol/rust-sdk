/**
 * ClientInfo represents information about the client, such as protocol version, capabilities, and implementation details.
 * This structure should mirror the Rust/Python SDK structure and will eventually be backed by WASM bindings.
 */
export interface ClientInfo {
  protocolVersion: string;
  capabilities: object; // Should be typed to ClientCapabilities
  clientInfo: Implementation;
}

/**
 * Implementation details for the client, such as name and version.
 * This structure should mirror the Rust/Python SDK Implementation struct.
 */
export interface Implementation {
  name: string;
  version: string;
}

/**
 * SSETransport provides an interface for connecting to an SSE (Server-Sent Events) endpoint.
 * In the browser, this would use EventSource; in Node.js, a compatible polyfill or HTTP client.
 * In the future, this should be backed by Rust/WASM for protocol logic.
 */
export class SSETransport {
  private url: string;
  private eventSource: EventSource | null = null;

  /**
   * Create a new SSETransport for the given URL.
   * @param url The SSE endpoint URL.
   */
  constructor(url: string) {
    this.url = url;
  }

  /**
   * Start the SSE connection. In browser, uses EventSource. In Node, requires a polyfill.
   * @param onMessage Callback for each message event.
   * @param onError Callback for error events.
   */
  start(onMessage: (data: any) => void, onError?: (err: any) => void) {
    if (typeof window !== 'undefined' && typeof window.EventSource !== 'undefined') {
      this.eventSource = new window.EventSource(this.url);
      this.eventSource.onmessage = (event) => {
        onMessage(event.data);
      };
      if (onError) {
        this.eventSource.onerror = onError;
      }
    } else {
      // Node.js: User must provide a compatible EventSource polyfill
      throw new Error('SSETransport requires EventSource (browser) or a polyfill (Node.js)');
    }
  }

  /**
   * Close the SSE connection.
   */
  close() {
    if (this.eventSource) {
      this.eventSource.close();
      this.eventSource = null;
    }
  }
}

/**
 * IntoTransport is an interface for types that can be converted into a transport.
 * This is a placeholder for extensibility and should mirror the Rust trait.
 */
export interface IntoTransport {
  intoTransport(): SSETransport;
}

// TODO: When Rust/WASM bindings are available, replace these stubs with real implementations and types.
