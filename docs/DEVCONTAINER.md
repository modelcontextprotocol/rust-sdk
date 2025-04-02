## Development with Dev Container and GitHub Codespaces

This repository provides a Dev Container to easily set up a development environment. Using Dev Container allows you to work in a consistent development environment with pre-configured dependencies and tools, whether locally or in the cloud with GitHub Codespaces.

### Prerequisites

**For Local Development:**

* [Docker Desktop](https://www.docker.com/products/docker-desktop/) or any other compatible container runtime (e.g., Podman, OrbStack) installed.
* [Visual Studio Code](https://code.visualstudio.com/) with the [Remote - Containers extension](https://marketplace.visualstudio.com/items?itemName=ms-vscode-remote.remote-containers) installed.

**For GitHub Codespaces:**

* A GitHub account.

### Starting Dev Container

**Using Visual Studio Code (Local):**

1.  Clone the repository.
2.  Open the repository in Visual Studio Code.
3.  Open the command palette in Visual Studio Code (`Ctrl + Shift + P` or `Cmd + Shift + P`) and execute `Dev Containers: Reopen in Container`.

**Using GitHub Codespaces (Cloud):**

1.  Navigate to the repository on GitHub.
2.  Click the "<> Code" button.
3.  Select the "Codespaces" tab.
4.  Click "Create codespace on main" (or your desired branch).

### Dev Container Configuration

Dev Container settings are configured in `.devcontainer/devcontainer.json`. In this file, you can set the Docker image to use, extensions to install, port forwarding, and more. This configuration is used both for local development and GitHub Codespaces.

### Development

Once the Dev Container is started, you can proceed with development as usual. The container already has the necessary tools and libraries installed. In GitHub Codespaces, you will have a fully configured VS Code in your browser or desktop application.

### Stopping Dev Container

**Using Visual Studio Code (Local):**

To stop the Dev Container, open the command palette in Visual Studio Code and execute `Remote: Close Remote Connection`.

**Using GitHub Codespaces (Cloud):**

GitHub Codespaces will automatically stop after a period of inactivity. You can also manually stop the codespace from the Codespaces menu in GitHub.

### More Information

* [Visual Studio Code Dev Containers](https://code.visualstudio.com/docs/remote/containers)
* [Dev Container Specification](https://containers.dev/implementors/json_reference/)
* [GitHub Codespaces](https://github.com/features/codespaces)

This document describes the basic usage of Dev Container and GitHub Codespaces. Add project-specific settings and procedures as needed.