# tree-sitter-ply

A tree-sitter grammar for Ply, so editors can highlight and indent Ply code. It
is a second parser beside the one in `packages/compiler`, so a syntax change
that is not made here too fails its checks.

| path | holds |
| --- | --- |
| `grammar.js` | the grammar: tokens and parsing rules |
| `src/` | the output of `tree-sitter generate`, including the C parser; committed so users need no build step |
| `queries/highlights.scm` | highlighting queries, using the standard capture names |
| `queries/indents.scm` | indentation queries that match `ply fmt`'s layout |
| `test/corpus/` | test cases for `tree-sitter test` |
| `scripts/parse-modules.sh` | parses every Ply file in the repository and fails on a syntax error |

## Working on it

```sh
npm install
npx tree-sitter generate   # rewrites src/, which is committed
npx tree-sitter test
scripts/parse-modules.sh
```

`src/parser.c` is generated and committed. CI fails if `grammar.js` changes
without running `tree-sitter generate`.

## Neovim

Neovim reads a parser from `parser/<lang>.<ext>` and queries from
`queries/<lang>/` on its `runtimepath`. Build the parser and copy the queries in:

```sh
npx tree-sitter build -o ~/.config/nvim/parser/ply.so
mkdir -p ~/.config/nvim/queries/ply
cp queries/highlights.scm queries/indents.scm ~/.config/nvim/queries/ply/
```

Then set the filetype, start the parser, and turn on indentation. The `.ply`
extension is also used by the Polygon File Format (3D meshes), whose files start
with a line holding `ply`, so the filetype rule skips those:

```vim
" ~/.config/nvim/ftdetect/ply.vim
au BufReadPost,BufNewFile *.ply if getline(1) !=# 'ply' | setfiletype ply | endif
```

```lua
-- ~/.config/nvim/ftplugin/ply.lua: two spaces per level, as `ply fmt` writes
vim.bo.expandtab = true
vim.bo.shiftwidth = 2
vim.treesitter.start()
require('nvim-treesitter.indent').attach(0)
```

In Neovim, `indents.scm` needs nvim-treesitter's indent module. Editors that
read `indents.scm` themselves (Helix, Zed) need neither that nor
`vim.treesitter.start()`. Instead of copying files, you can put a checkout of
this directory on `runtimepath`. nvim-treesitter does not know about Ply, which
is why the ftplugin starts the parser itself.

## GitHub

GitHub highlights code with TextMate grammars through Linguist, not with
tree-sitter, so this grammar does not help there. Linguist adds a language only
once many public repositories use it, and Ply would also need a TextMate grammar
and a way to tell its files from 3D meshes.

## License

MIT, like the rest of the repository.
