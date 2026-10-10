# tree-sitter-ply

A tree-sitter grammar for Ply. It is a second reading of the language beside
`crates/ply-compiler/ply`, written so editors highlight Ply and so a rule the
language gains or drops fails a check here.

| path | holds |
| --- | --- |
| `grammar.js` | the grammar: the lexer's tokens and the parser's rules |
| `src/` | what `tree-sitter generate` writes, the C parser among it, committed so a consumer builds nothing |
| `queries/highlights.scm` | the highlight queries, in the captures every tree-sitter editor reads |
| `test/corpus/` | the cases `tree-sitter test` holds the tree to |
| `scripts/parse-corpus.sh` | every Ply module in this tree, parsed with the grammar and refused on a syntax error |

## Working on it

```sh
npm install
npx tree-sitter generate   # rewrites src/, which is committed
npx tree-sitter test
scripts/parse-corpus.sh    # needs no argument; finds the tree from its own path
```

`src/parser.c` is generated and committed, so a consumer needs no toolchain: a
change to `grammar.js` that is not followed by `tree-sitter generate` fails CI.

## Neovim

Neovim reads a parser from `parser/<lang>.<ext>` and queries from
`queries/<lang>/` on its `runtimepath`. Build the parser and copy both in:

```sh
npx tree-sitter build -o ~/.config/nvim/parser/ply.so
mkdir -p ~/.config/nvim/queries/ply
cp queries/highlights.scm ~/.config/nvim/queries/ply/
```

Then map the filetype and start the parser on it, in
`~/.config/nvim/ftdetect/ply.vim` and a `FileType` autocmd:

```vim
au BufRead,BufNewFile *.ply setf ply
au FileType ply lua vim.treesitter.start()
```

A checkout of this directory on `runtimepath` serves the same files, since
Neovim finds `queries/` under it; `nvim-treesitter`'s archived `master` branch
does not know `ply`, so the `FileType` autocmd above is what starts it.

The `.ply` extension is also the Polygon File Format's (a 3D mesh), which some
editors detect by content or by name; a file that opens as a mesh needs the
filetype set by hand.

## GitHub

GitHub highlights through a TextMate grammar in Linguist, not tree-sitter, so
this grammar does not reach it. Linguist takes a language once enough public
`.ply` files exist (about two thousand, over two hundred repositories) and reads
a sample set, which the 3D meshes under the same extension complicate; a
TextMate grammar is the other half of that work.

## License

MIT, as the repository.
