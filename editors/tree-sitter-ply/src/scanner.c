#include "tree_sitter/parser.h"

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
          lexer->mark_end(lexer);
          lexer->result_symbol = LABEL;
          return true;
        }
        lexer->mark_end(lexer);
        candidate = true;
        scan_nested_string(lexer);
        continue;
      }
      lexer->advance(lexer, false);
    }
    if (!candidate) {
      lexer->mark_end(lexer);
    }
    lexer->result_symbol = LABEL;
    return true;
  }
  return false;
}
