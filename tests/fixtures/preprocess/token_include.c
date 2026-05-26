#define HEADER_NAME "token_include_header.h"
#define INCLUDE_TARGET(x) x
#define PUNCT_HEADER "punct-dir/header.name-v1.h"
#include INCLUDE_TARGET(HEADER_NAME)
#include PUNCT_HEADER
#include "space dir/header with spaces.h"
int from_macro_include = INCLUDED_VALUE;
int from_macro_punct_include = PUNCT_INCLUDED_VALUE;
int from_spaced_include = SPACED_INCLUDED_VALUE;
