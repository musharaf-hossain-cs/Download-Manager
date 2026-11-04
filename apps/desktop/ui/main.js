const statusCard = globalThis.document.querySelector(".status-card");
const statusTitle = globalThis.document.querySelector("#status-title");
const statusMessage = globalThis.document.querySelector("#status-message");
const backendVersion = globalThis.document.querySelector("#backend-version");

function showFailure(message) {
  statusCard.dataset.state = "error";
  statusCard.setAttribute("role", "alert");
  statusTitle.textContent = "The local backend could not start";
  statusMessage.textContent = message;
  backendVersion.hidden = true;
}

async function showBackendStatus() {
  try {
    const status = await globalThis.__TAURI__.core.invoke("backend_status");
    if (!status.ready) {
      showFailure(status.message);
      return;
    }

    statusCard.dataset.state = "ready";
    statusTitle.textContent = "Ready";
    statusMessage.textContent = status.message;
    backendVersion.textContent = `Backend ${status.version}`;
    backendVersion.hidden = false;
  } catch (error) {
    showFailure(`The desktop shell could not read backend status: ${error}`);
  }
}

void showBackendStatus();
