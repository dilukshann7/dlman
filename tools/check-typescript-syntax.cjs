/** Parse changed TypeScript without emitting files or starting a bundler. */
const fs = require('node:fs');
const cp = require('node:child_process');
const compiler = process.env.DLMAN_TYPESCRIPT_PATH || 'typescript';
const ts = require(compiler);
const files = [...new Set(cp.execFileSync('git', ['ls-files', '--modified', '--others', '--exclude-standard'], { encoding: 'utf8' })
  .trim().split(/\r?\n/))].filter((file) => /\.tsx?$/.test(file));
let errors = 0;
for (const file of files) {
  const result = ts.transpileModule(fs.readFileSync(file, 'utf8'), { fileName: file, reportDiagnostics: true,
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } });
  for (const diagnostic of result.diagnostics || []) {
    if (diagnostic.category !== ts.DiagnosticCategory.Error) continue;
    errors++;
    process.stdout.write(`${file}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n')}\n`);
  }
}
process.stdout.write(`TypeScript syntax: ${files.length} files, ${errors} errors\n`);
process.exitCode = errors ? 1 : 0;
