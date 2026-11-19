import init from "/assets/graph_map_ui.js";

const note = document.getElementById("connecting");
init().catch(() => {
  if (note) {
    note.textContent = "Could not load the graph-map interface. Run from localhost and refresh.";
    note.hidden = false;
  }
});
