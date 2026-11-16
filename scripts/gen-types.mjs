#!/usr/bin/env node
/**
 * Generates `apps/web/src/lib/types.gen.ts` from the server's OpenAPI document
 * (`ludex-server openapi`).
 *
 * Usage:
 *   node scripts/gen-types.mjs           # check the committed file is current
 *   node scripts/gen-types.mjs --write   # regenerate it
 *   node scripts/gen-types.mjs --document openapi.json [--write]
 *
 * The conversion is intentionally small: it covers the JSON Schema subset the
 * server's `utoipa` derives emit (objects, refs, arrays, enums, nullable
 * unions and `allOf` intersections), with no dependency to install.
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = resolve(root, "apps/web/src/lib/types.gen.ts");
const args = process.argv.slice(2);
const write = args.includes("--write");
const documentIndex = args.indexOf("--document");

function loadDocument() {
  if (documentIndex !== -1) {
    return JSON.parse(readFileSync(resolve(root, args[documentIndex + 1]), "utf8"));
  }
  const stdout = execFileSync(
    "cargo",
    ["run", "--quiet", "-p", "ludex-server", "--bin", "ludex-server", "--", "openapi"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  return JSON.parse(stdout);
}

const refName = (ref) => {
  const prefix = "#/components/schemas/";
  if (!ref.startsWith(prefix)) throw new Error(`unsupported $ref ${ref}`);
  return ref.slice(prefix.length);
};

const literal = (value) => JSON.stringify(value);

function typeOf(schema, indent) {
  if (!schema || typeof schema !== "object") return "unknown";
  if (schema.$ref) return refName(schema.$ref);
  if (schema.oneOf) {
    const members = schema.oneOf.map((member) => typeOf(member, indent));
    return [...new Set(members)].join(" | ");
  }
  if (schema.anyOf) {
    const members = schema.anyOf.map((member) => typeOf(member, indent));
    return [...new Set(members)].join(" | ");
  }
  if (schema.allOf) {
    return schema.allOf.map((member) => typeOf(member, indent)).join(" & ");
  }
  if (Array.isArray(schema.type)) {
    const members = schema.type.map((type) => typeOf({ ...schema, type }, indent));
    return [...new Set(members)].join(" | ");
  }
  if (schema.enum) return schema.enum.map(literal).join(" | ");
  if (schema.const !== undefined) return literal(schema.const);
  switch (schema.type) {
    case "string":
      return "string";
    case "integer":
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "null":
      return "null";
    case "array": {
      const item = typeOf(schema.items, indent);
      return item.includes(" ") ? `Array<${item}>` : `${item}[]`;
    }
    case "object":
    case undefined: {
      if (schema.properties) return objectLiteral(schema, indent);
      if (schema.additionalProperties && typeof schema.additionalProperties === "object") {
        return `Record<string, ${typeOf(schema.additionalProperties, indent)}>`;
      }
      return "Record<string, unknown>";
    }
    default:
      throw new Error(`unsupported schema type ${JSON.stringify(schema.type)}`);
  }
}

function objectLiteral(schema, indent) {
  const required = new Set(schema.required ?? []);
  const lines = [];
  for (const name of Object.keys(schema.properties).sort()) {
    const property = schema.properties[name];
    const optional = required.has(name) ? "" : "?";
    lines.push(`${indent}  ${name}${optional}: ${typeOf(property, `${indent}  `)};`);
  }
  if (schema.additionalProperties === true) lines.push(`${indent}  [key: string]: unknown;`);
  return lines.length ? `{\n${lines.join("\n")}\n${indent}}` : "Record<string, unknown>";
}

function declaration(name, schema) {
  if (schema.allOf || schema.oneOf || schema.anyOf || schema.enum || Array.isArray(schema.type)) {
    return `export type ${name} = ${typeOf(schema, "")};`;
  }
  if (schema.type === "object" && schema.properties) {
    const required = new Set(schema.required ?? []);
    const lines = Object.keys(schema.properties)
      .sort()
      .map((property) => {
        const optional = required.has(property) ? "" : "?";
        return `  ${property}${optional}: ${typeOf(schema.properties[property], "  ")};`;
      });
    return `export interface ${name} {\n${lines.join("\n")}\n}`;
  }
  return `export type ${name} = ${typeOf(schema, "")};`;
}

function render(document) {
  const schemas = document.components?.schemas;
  if (!schemas) throw new Error("document has no components.schemas");
  const header = [
    "// This file is generated from the server OpenAPI document.",
    "// Regenerate with `node scripts/gen-types.mjs --write`; CI fails when it is stale.",
    "// Do not edit by hand.",
    "",
  ].join("\n");
  const body = Object.keys(schemas)
    .sort()
    .map((name) => declaration(name, schemas[name]))
    .filter(Boolean)
    .join("\n\n");
  return `${header}\n${body}\n`;
}

const generated = render(loadDocument());
if (write) {
  writeFileSync(outputPath, generated);
  console.log(`wrote ${outputPath}`);
} else {
  let current = "";
  try {
    current = readFileSync(outputPath, "utf8");
  } catch {
    console.error(`${outputPath} is missing; run node scripts/gen-types.mjs --write`);
    process.exit(1);
  }
  if (current !== generated) {
    console.error(`${outputPath} is stale; run node scripts/gen-types.mjs --write and commit the result.`);
    process.exit(1);
  }
  console.log(`${outputPath} is up to date`);
}
