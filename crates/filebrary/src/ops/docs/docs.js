// Renders /api/openapi.json as a reference page. Uses only textContent, so
// nothing in the document is interpreted as HTML.
"use strict";

const METHODS = ["get", "post", "put", "patch", "delete"];

function el(tag, attrs, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === "class") e.className = v; else e.setAttribute(k, v);
  }
  for (const c of children) {
    if (c == null) continue;
    e.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return e;
}

function resolve(doc, schema) {
  if (schema && schema.$ref) {
    const name = schema.$ref.split("/").pop();
    return [name, doc.components.schemas[name] || {}];
  }
  return [null, schema || {}];
}

// Builds an example-like JSON skeleton of a schema, to show its shape.
function shape(doc, schema, depth) {
  const [, s] = resolve(doc, schema);
  if (depth > 5) return "…";
  if (s.example !== undefined) return s.example;
  if (s.oneOf || s.anyOf) {
    const alts = (s.oneOf || s.anyOf).filter((a) => a.type !== "null");
    return alts.length ? shape(doc, alts[0], depth + 1) : null;
  }
  if (s.enum) return s.enum.join(" | ");
  let type = s.type;
  if (Array.isArray(type)) type = type.find((t) => t !== "null");
  switch (type) {
    case "object": {
      const out = {};
      for (const [k, v] of Object.entries(s.properties || {})) out[k] = shape(doc, v, depth + 1);
      return out;
    }
    case "array": return [shape(doc, s.items, depth + 1)];
    case "integer": case "number": return 0;
    case "boolean": return false;
    case "string": return s.format === "date-time" ? "2026-01-01T00:00:00Z" : "string";
    default: return s.properties ? shape(doc, { ...s, type: "object" }, depth) : null;
  }
}

function schemaBlock(doc, schema) {
  const [name] = resolve(doc, schema);
  const pre = el("pre", {}, JSON.stringify(shape(doc, schema, 0), null, 2));
  return name ? el("div", {}, el("code", {}, name), pre) : pre;
}

function fieldsTable(doc, schema) {
  const [, s] = resolve(doc, schema);
  if (!s.properties) return null;
  const req = new Set(s.required || []);
  const rows = Object.entries(s.properties).map(([k, v]) => {
    const [ref, r] = resolve(doc, v);
    const type = ref || (Array.isArray(r.type) ? r.type.join(" | ") : r.type) || "";
    return el("tr", {}, el("td", {}, el("code", {}, k)), el("td", {}, type + (req.has(k) ? "" : " (optional)")),
      el("td", {}, r.description || v.description || ""));
  });
  return el("table", {}, el("tr", {}, el("th", {}, "Field"), el("th", {}, "Type"), el("th", {}, "Description")), ...rows);
}

function operation(doc, path, method, op) {
  const body = el("div", { class: "body" });
  if (op.description) body.append(el("p", {}, op.description));
  const sec = op.security || doc.security || [];
  const schemes = sec.flatMap((s) => Object.keys(s));
  body.append(el("p", { class: "lock" }, schemes.length ? "Auth: " + schemes.join(", ") : "No authentication"));
  if (op.parameters && op.parameters.length) {
    body.append(el("h4", {}, "Parameters"));
    body.append(el("table", {},
      el("tr", {}, el("th", {}, "Name"), el("th", {}, "In"), el("th", {}, "Description")),
      ...op.parameters.map((p) => el("tr", {}, el("td", {}, el("code", {}, p.name)),
        el("td", {}, p.in + (p.required ? "" : " (optional)")), el("td", {}, p.description || "")))));
  }
  if (op.requestBody) {
    body.append(el("h4", {}, "Request body"));
    for (const [ct, media] of Object.entries(op.requestBody.content || {})) {
      body.append(el("p", {}, el("code", {}, ct)));
      if (media.schema) {
        body.append(schemaBlock(doc, media.schema));
        const t = fieldsTable(doc, media.schema);
        if (t) body.append(t);
      }
    }
  }
  body.append(el("h4", {}, "Responses"));
  for (const [code, r] of Object.entries(op.responses || {})) {
    body.append(el("p", {}, el("strong", {}, code + " "), r.description || ""));
    for (const media of Object.values(r.content || {})) {
      if (media.schema) body.append(schemaBlock(doc, media.schema));
    }
  }
  const text = (path + " " + (op.summary || "") + " " + (op.description || "")).toLowerCase();
  const d = el("details", { "data-text": text },
    el("summary", {}, el("span", { class: "method m-" + method }, method),
      el("span", { class: "path" }, path), el("span", { class: "summary" }, op.summary || "")),
    body);
  return d;
}

async function main() {
  const content = document.getElementById("content");
  let doc;
  try {
    const r = await fetch("/api/openapi.json", { credentials: "same-origin" });
    doc = await r.json();
  } catch (e) {
    document.getElementById("meta").textContent = "Could not load /api/openapi.json.";
    return;
  }
  document.getElementById("meta").textContent = "Version " + doc.info.version + " · OpenAPI " + doc.openapi;
  content.append(el("p", { class: "intro" }, doc.info.description || ""));
  const byTag = new Map((doc.tags || []).map((t) => [t.name, { desc: t.description, ops: [] }]));
  for (const [path, item] of Object.entries(doc.paths || {})) {
    for (const m of METHODS) {
      if (!item[m]) continue;
      const tag = (item[m].tags || ["other"])[0];
      if (!byTag.has(tag)) byTag.set(tag, { desc: "", ops: [] });
      byTag.get(tag).ops.push(operation(doc, path, m, item[m]));
    }
  }
  for (const [tag, { desc, ops }] of byTag) {
    if (!ops.length) continue;
    const section = el("section", {}, el("h2", {}, tag), desc ? el("p", { class: "tagdesc" }, desc) : null, ...ops);
    content.append(section);
  }
  document.getElementById("filter").addEventListener("input", (ev) => {
    const q = ev.target.value.trim().toLowerCase();
    for (const d of content.querySelectorAll("details")) {
      d.hidden = q !== "" && !d.dataset.text.includes(q);
    }
    for (const s of content.querySelectorAll("section")) {
      s.hidden = [...s.querySelectorAll("details")].every((d) => d.hidden);
    }
  });
}

main();
