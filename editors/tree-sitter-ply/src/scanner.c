#include "tree_sitter/parser.h"

// Two tokens the lexer cannot decide on its own.
//
// `GT`: a `>` that closes a type argument. Ply lexes `>>` as two tokens and joins adjacent `>`s
// when closing types, so a grammar that keeps `>>` and `>>>` as expression tokens must hand the
// type context one `>` at a time. The scanner runs only where `_gt` can be accepted, so an
// expression's `>` and `>>` are untouched.
//
// `LABEL`: the quoted name of a `test` or `law`. A test's label is an interpolated string only when
// a `for` follows it, and reading it either way then needs the other's tokens; the scanner reads the
// whole quoted string (holes, nested strings and escapes included) and hands it over as one token.
enum TokenType {
  GT,
  LABEL,
};

void *tree_sitter_ply_external_scanner_create(void) {
  return NULL;
}

void tree_sitter_ply_external_scanner_destroy(void *payload) {
}

unsigned tree_sitter_ply_external_scanner_serialize(void *payload, char *buffer) {
  return 0;
}

void tree_sitter_ply_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
}

static void skip_space(TSLexer *lexer) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\r' ||
         lexer->lookahead == '\n' || lexer->lookahead == '\f' || lexer->lookahead == '\v') {
    lexer->advance(lexer, true);
  }
}

static void scan_nested_string(TSLexer *lexer) {
  while (lexer->lookahead != 0 && lexer->lookahead != '"' && lexer->lookahead != '\n') {
    if (lexer->lookahead == '\\') {
      lexer->advance(lexer, false);
      if (lexer->lookahead != 0 && lexer->lookahead != '\n') {
        lexer->advance(lexer, false);
      }
      continue;
    }
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead == '"') {
    lexer->advance(lexer, false);
  }
}

bool tree_sitter_ply_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  skip_space(lexer);
  if (valid_symbols[GT]) {
    if (lexer->lookahead == '>') {
      lexer->advance(lexer, false);
      lexer->result_symbol = GT;
      return true;
    }
  }
  if (valid_symbols[LABEL] && lexer->lookahead == '"') {
    lexer->advance(lexer, false);
    int depth = 0;
    bool candidate = false;
    while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
      int c = lexer->lookahead;
      if (c == '\\') {
        lexer->advance(lexer, false);
        if (lexer->lookahead != 0 && lexer->lookahead != '\n') {
          lexer->advance(lexer, false);
        }
        continue;
      }
      if (c == '{') {
        depth++;
        lexer->advance(lexer, false);
        continue;
      }
      if (c == '}') {
        if (depth > 0) {
          depth--;
        }
        lexer->advance(lexer, false);
        continue;
      }
      if (c == '"') {
        lexer->advance(lexer, false);
        if (depth == 0) {
          // The end of both a plain string and a template.
          lexer->mark_end(lexer);
          lexer->result_symbol = LABEL;
          return true;
        }
        // Inside a hole a `"` opens a nested string; if the template never closes, this quote was
        // the end of a plain string, and the last `mark_end` before it keeps that end.
        lexer->mark_end(lexer);
        candidate = true;
        scan_nested_string(lexer);
        continue;
      }
      lexer->advance(lexer, false);
    }
    // Unterminated: hand the rest of the line over as the label so recovery keeps its place.
    if (!candidate) {
      lexer->mark_end(lexer);
    }
    lexer->result_symbol = LABEL;
    return true;
  }
  return false;
}
