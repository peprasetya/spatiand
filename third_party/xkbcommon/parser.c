/* A Bison parser, made by GNU Bison 3.8.2.  */

/* Bison implementation for Yacc-like parsers in C

   Copyright (C) 1984, 1989-1990, 2000-2015, 2018-2021 Free Software Foundation,
   Inc.

   This program is free software: you can redistribute it and/or modify
   it under the terms of the GNU General Public License as published by
   the Free Software Foundation, either version 3 of the License, or
   (at your option) any later version.

   This program is distributed in the hope that it will be useful,
   but WITHOUT ANY WARRANTY; without even the implied warranty of
   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
   GNU General Public License for more details.

   You should have received a copy of the GNU General Public License
   along with this program.  If not, see <https://www.gnu.org/licenses/>.  */

/* As a special exception, you may create a larger work that contains
   part or all of the Bison parser skeleton and distribute that work
   under terms of your choice, so long as that work isn't itself a
   parser generator using the skeleton or a modified version thereof
   as a parser skeleton.  Alternatively, if you modify or redistribute
   the parser skeleton itself, you may (at your option) remove this
   special exception, which will cause the skeleton and the resulting
   Bison output files to be licensed under the GNU General Public
   License without this special exception.

   This special exception was added by the Free Software Foundation in
   version 2.2 of Bison.  */

/* C LALR(1) parser skeleton written by Richard Stallman, by
   simplifying the original so-called "semantic" parser.  */

/* DO NOT RELY ON FEATURES THAT ARE NOT DOCUMENTED in the manual,
   especially those whose name start with YY_ or yy_.  They are
   private implementation details that can be changed or removed.  */

/* All symbols defined below should begin with yy or YY, to avoid
   infringing on user name space.  This should be done even for local
   variables, as they might otherwise be expanded by user macros.
   There are some unavoidable exceptions within include files to
   define necessary library symbols; they are noted "INFRINGES ON
   USER NAME SPACE" below.  */

/* Identify Bison output, and Bison version.  */
#define YYBISON 30802

/* Bison version string.  */
#define YYBISON_VERSION "3.8.2"

/* Skeleton name.  */
#define YYSKELETON_NAME "yacc.c"

/* Pure parsers.  */
#define YYPURE 1

/* Push parsers.  */
#define YYPUSH 0

/* Pull parsers.  */
#define YYPULL 1


/* Substitute the variable and function names.  */
#define yyparse         _xkbcommon_parse
#define yylex           _xkbcommon_lex
#define yyerror         _xkbcommon_error
#define yydebug         _xkbcommon_debug
#define yynerrs         _xkbcommon_nerrs

/* First part of user prologue.  */
#line 21 "src/xkbcomp/parser.y"

#include "config.h"

#include <assert.h>

#include "keysym.h"
#include "xkbcomp/xkbcomp-priv.h"
#include "xkbcomp/parser-priv.h"
#include "xkbcomp/ast-build.h"

struct parser_param {
    struct xkb_context *ctx;
    struct scanner *scanner;
    XkbFile *rtrn;
    bool more_maps;
};

#define parser_err(param, error_id, fmt, ...) \
    scanner_err((param)->scanner, error_id, fmt, ##__VA_ARGS__)

#define parser_warn(param, warning_id, fmt, ...) \
    scanner_warn((param)->scanner, warning_id, fmt, ##__VA_ARGS__)

#define parser_vrb(param, verbosity, warning_id, fmt, ...) \
    scanner_vrb((param)->scanner, verbosity, warning_id, fmt, ##__VA_ARGS__)

static void
_xkbcommon_error(struct parser_param *param, const char *msg)
{
    parser_err(param, XKB_ERROR_INVALID_XKB_SYNTAX, "%s", msg);
}

static bool
resolve_keysym(struct parser_param *param, struct sval name, xkb_keysym_t *sym_rtrn)
{
    xkb_keysym_t sym;

    if (isvaleq(name, SVAL_LIT("any")) || isvaleq(name, SVAL_LIT("nosymbol"))) {
        *sym_rtrn = XKB_KEY_NoSymbol;
        return true;
    }

    if (isvaleq(name, SVAL_LIT("none")) || isvaleq(name, SVAL_LIT("voidsymbol"))) {
        *sym_rtrn = XKB_KEY_VoidSymbol;
        return true;
    }

    /* xkb_keysym_from_name needs a C string. */
    char buf[XKB_KEYSYM_NAME_MAX_SIZE];
    if (name.len >= sizeof(buf)) {
        return false;
    }
    memcpy(buf, name.start, name.len);
    buf[name.len] = '\0';

    sym = xkb_keysym_from_name(buf, XKB_KEYSYM_NO_FLAGS);
    if (sym != XKB_KEY_NoSymbol) {
        *sym_rtrn = sym;
        check_deprecated_keysyms(parser_warn, param, param->ctx,
                                 sym, buf, buf, "%s", "");
        return true;
    }

    return false;
}

#define param_scanner param->scanner

#line 145 "parser.c"

# ifndef YY_CAST
#  ifdef __cplusplus
#   define YY_CAST(Type, Val) static_cast<Type> (Val)
#   define YY_REINTERPRET_CAST(Type, Val) reinterpret_cast<Type> (Val)
#  else
#   define YY_CAST(Type, Val) ((Type) (Val))
#   define YY_REINTERPRET_CAST(Type, Val) ((Type) (Val))
#  endif
# endif
# ifndef YY_NULLPTR
#  if defined __cplusplus
#   if 201103L <= __cplusplus
#    define YY_NULLPTR nullptr
#   else
#    define YY_NULLPTR 0
#   endif
#  else
#   define YY_NULLPTR ((void*)0)
#  endif
# endif

#include "parser.h"
/* Symbol kind.  */
enum yysymbol_kind_t
{
  YYSYMBOL_YYEMPTY = -2,
  YYSYMBOL_YYEOF = 0,                      /* "end of file"  */
  YYSYMBOL_YYerror = 1,                    /* error  */
  YYSYMBOL_YYUNDEF = 2,                    /* "invalid token"  */
  YYSYMBOL_ERROR_TOK = 3,                  /* "invalid token"  */
  YYSYMBOL_XKB_KEYMAP = 4,                 /* "xkb_keymap"  */
  YYSYMBOL_XKB_KEYCODES = 5,               /* "xkb_keycodes"  */
  YYSYMBOL_XKB_TYPES = 6,                  /* "xkb_types"  */
  YYSYMBOL_XKB_SYMBOLS = 7,                /* "xkb_symbols"  */
  YYSYMBOL_XKB_COMPATMAP = 8,              /* "xkb_compatibility"  */
  YYSYMBOL_XKB_GEOMETRY = 9,               /* "xkb_geometry"  */
  YYSYMBOL_XKB_SEMANTICS = 10,             /* "xkb_semantics"  */
  YYSYMBOL_XKB_LAYOUT = 11,                /* "xkb_layout"  */
  YYSYMBOL_INCLUDE = 12,                   /* "include"  */
  YYSYMBOL_OVERRIDE = 13,                  /* "override"  */
  YYSYMBOL_AUGMENT = 14,                   /* "augment"  */
  YYSYMBOL_REPLACE = 15,                   /* "replace"  */
  YYSYMBOL_ALTERNATE = 16,                 /* "alternate"  */
  YYSYMBOL_VIRTUAL_MODS = 17,              /* "virtual_modifiers"  */
  YYSYMBOL_TYPE = 18,                      /* "type"  */
  YYSYMBOL_INTERPRET = 19,                 /* "interpret"  */
  YYSYMBOL_ACTION_TOK = 20,                /* "action"  */
  YYSYMBOL_KEY = 21,                       /* "key"  */
  YYSYMBOL_ALIAS = 22,                     /* "alias"  */
  YYSYMBOL_GROUP = 23,                     /* "group"  */
  YYSYMBOL_MODIFIER_MAP = 24,              /* "modifier_map"  */
  YYSYMBOL_INDICATOR = 25,                 /* "indicator"  */
  YYSYMBOL_SHAPE = 26,                     /* "shape"  */
  YYSYMBOL_KEYS = 27,                      /* "keys"  */
  YYSYMBOL_ROW = 28,                       /* "row"  */
  YYSYMBOL_SECTION = 29,                   /* "section"  */
  YYSYMBOL_OVERLAY = 30,                   /* "overlay"  */
  YYSYMBOL_TEXT = 31,                      /* "text"  */
  YYSYMBOL_OUTLINE = 32,                   /* "outline"  */
  YYSYMBOL_SOLID = 33,                     /* "solid"  */
  YYSYMBOL_LOGO = 34,                      /* "logo"  */
  YYSYMBOL_VIRTUAL = 35,                   /* "virtual"  */
  YYSYMBOL_EQUALS = 36,                    /* "="  */
  YYSYMBOL_PLUS = 37,                      /* "+"  */
  YYSYMBOL_MINUS = 38,                     /* "-"  */
  YYSYMBOL_DIVIDE = 39,                    /* "/"  */
  YYSYMBOL_TIMES = 40,                     /* "*"  */
  YYSYMBOL_OBRACE = 41,                    /* "{"  */
  YYSYMBOL_CBRACE = 42,                    /* "}"  */
  YYSYMBOL_OPAREN = 43,                    /* "("  */
  YYSYMBOL_CPAREN = 44,                    /* ")"  */
  YYSYMBOL_OBRACKET = 45,                  /* "["  */
  YYSYMBOL_CBRACKET = 46,                  /* "]"  */
  YYSYMBOL_DOT = 47,                       /* "."  */
  YYSYMBOL_COMMA = 48,                     /* ","  */
  YYSYMBOL_SEMI = 49,                      /* ";"  */
  YYSYMBOL_EXCLAM = 50,                    /* "!"  */
  YYSYMBOL_INVERT = 51,                    /* "~"  */
  YYSYMBOL_STRING = 52,                    /* "string literal"  */
  YYSYMBOL_DECIMAL_DIGIT = 53,             /* "decimal digit"  */
  YYSYMBOL_INTEGER = 54,                   /* "integer literal"  */
  YYSYMBOL_FLOAT = 55,                     /* "float literal"  */
  YYSYMBOL_IDENT = 56,                     /* "identifier"  */
  YYSYMBOL_KEYNAME = 57,                   /* "key name"  */
  YYSYMBOL_PARTIAL = 58,                   /* "partial"  */
  YYSYMBOL_DEFAULT = 59,                   /* "default"  */
  YYSYMBOL_HIDDEN = 60,                    /* "hidden"  */
  YYSYMBOL_ALPHANUMERIC_KEYS = 61,         /* "alphanumeric_keys"  */
  YYSYMBOL_MODIFIER_KEYS = 62,             /* "modifier_keys"  */
  YYSYMBOL_KEYPAD_KEYS = 63,               /* "keypad_keys"  */
  YYSYMBOL_FUNCTION_KEYS = 64,             /* "function_keys"  */
  YYSYMBOL_ALTERNATE_GROUP = 65,           /* "alternate_group"  */
  YYSYMBOL_YYACCEPT = 66,                  /* $accept  */
  YYSYMBOL_XkbFile = 67,                   /* XkbFile  */
  YYSYMBOL_XkbCompositeMap = 68,           /* XkbCompositeMap  */
  YYSYMBOL_XkbCompositeType = 69,          /* XkbCompositeType  */
  YYSYMBOL_XkbMapConfigList = 70,          /* XkbMapConfigList  */
  YYSYMBOL_XkbMapConfig = 71,              /* XkbMapConfig  */
  YYSYMBOL_FileType = 72,                  /* FileType  */
  YYSYMBOL_OptFlags = 73,                  /* OptFlags  */
  YYSYMBOL_Flags = 74,                     /* Flags  */
  YYSYMBOL_Flag = 75,                      /* Flag  */
  YYSYMBOL_DeclList = 76,                  /* DeclList  */
  YYSYMBOL_Decl = 77,                      /* Decl  */
  YYSYMBOL_VarDecl = 78,                   /* VarDecl  */
  YYSYMBOL_KeyNameDecl = 79,               /* KeyNameDecl  */
  YYSYMBOL_KeyAliasDecl = 80,              /* KeyAliasDecl  */
  YYSYMBOL_VModDecl = 81,                  /* VModDecl  */
  YYSYMBOL_VModDefList = 82,               /* VModDefList  */
  YYSYMBOL_VModDef = 83,                   /* VModDef  */
  YYSYMBOL_InterpretDecl = 84,             /* InterpretDecl  */
  YYSYMBOL_InterpretMatch = 85,            /* InterpretMatch  */
  YYSYMBOL_VarDeclList = 86,               /* VarDeclList  */
  YYSYMBOL_KeyTypeDecl = 87,               /* KeyTypeDecl  */
  YYSYMBOL_SymbolsDecl = 88,               /* SymbolsDecl  */
  YYSYMBOL_OptSymbolsBody = 89,            /* OptSymbolsBody  */
  YYSYMBOL_SymbolsBody = 90,               /* SymbolsBody  */
  YYSYMBOL_SymbolsVarDecl = 91,            /* SymbolsVarDecl  */
  YYSYMBOL_MultiKeySymOrActionList = 92,   /* MultiKeySymOrActionList  */
  YYSYMBOL_NoSymbolOrActionList = 93,      /* NoSymbolOrActionList  */
  YYSYMBOL_GroupCompatDecl = 94,           /* GroupCompatDecl  */
  YYSYMBOL_ModMapDecl = 95,                /* ModMapDecl  */
  YYSYMBOL_KeyOrKeySymList = 96,           /* KeyOrKeySymList  */
  YYSYMBOL_KeyOrKeySym = 97,               /* KeyOrKeySym  */
  YYSYMBOL_LedMapDecl = 98,                /* LedMapDecl  */
  YYSYMBOL_LedNameDecl = 99,               /* LedNameDecl  */
  YYSYMBOL_ShapeDecl = 100,                /* ShapeDecl  */
  YYSYMBOL_SectionDecl = 101,              /* SectionDecl  */
  YYSYMBOL_SectionBody = 102,              /* SectionBody  */
  YYSYMBOL_SectionBodyItem = 103,          /* SectionBodyItem  */
  YYSYMBOL_RowBody = 104,                  /* RowBody  */
  YYSYMBOL_RowBodyItem = 105,              /* RowBodyItem  */
  YYSYMBOL_Keys = 106,                     /* Keys  */
  YYSYMBOL_Key = 107,                      /* Key  */
  YYSYMBOL_OverlayDecl = 108,              /* OverlayDecl  */
  YYSYMBOL_OverlayKeyList = 109,           /* OverlayKeyList  */
  YYSYMBOL_OverlayKey = 110,               /* OverlayKey  */
  YYSYMBOL_OutlineList = 111,              /* OutlineList  */
  YYSYMBOL_OutlineInList = 112,            /* OutlineInList  */
  YYSYMBOL_CoordList = 113,                /* CoordList  */
  YYSYMBOL_Coord = 114,                    /* Coord  */
  YYSYMBOL_DoodadDecl = 115,               /* DoodadDecl  */
  YYSYMBOL_DoodadType = 116,               /* DoodadType  */
  YYSYMBOL_FieldSpec = 117,                /* FieldSpec  */
  YYSYMBOL_Element = 118,                  /* Element  */
  YYSYMBOL_OptMergeMode = 119,             /* OptMergeMode  */
  YYSYMBOL_MergeMode = 120,                /* MergeMode  */
  YYSYMBOL_ExprList = 121,                 /* ExprList  */
  YYSYMBOL_Expr = 122,                     /* Expr  */
  YYSYMBOL_Term = 123,                     /* Term  */
  YYSYMBOL_MultiActionList = 124,          /* MultiActionList  */
  YYSYMBOL_ActionList = 125,               /* ActionList  */
  YYSYMBOL_NonEmptyActions = 126,          /* NonEmptyActions  */
  YYSYMBOL_Actions = 127,                  /* Actions  */
  YYSYMBOL_Action = 128,                   /* Action  */
  YYSYMBOL_Lhs = 129,                      /* Lhs  */
  YYSYMBOL_Terminal = 130,                 /* Terminal  */
  YYSYMBOL_MultiKeySymList = 131,          /* MultiKeySymList  */
  YYSYMBOL_KeySymList = 132,               /* KeySymList  */
  YYSYMBOL_NonEmptyKeySyms = 133,          /* NonEmptyKeySyms  */
  YYSYMBOL_KeySyms = 134,                  /* KeySyms  */
  YYSYMBOL_KeySym = 135,                   /* KeySym  */
  YYSYMBOL_KeySymLit = 136,                /* KeySymLit  */
  YYSYMBOL_SignedNumber = 137,             /* SignedNumber  */
  YYSYMBOL_Number = 138,                   /* Number  */
  YYSYMBOL_Float = 139,                    /* Float  */
  YYSYMBOL_Integer = 140,                  /* Integer  */
  YYSYMBOL_KeyCode = 141,                  /* KeyCode  */
  YYSYMBOL_Ident = 142,                    /* Ident  */
  YYSYMBOL_String = 143,                   /* String  */
  YYSYMBOL_OptMapName = 144,               /* OptMapName  */
  YYSYMBOL_MapName = 145                   /* MapName  */
};
typedef enum yysymbol_kind_t yysymbol_kind_t;




#ifdef short
# undef short
#endif

/* On compilers that do not define __PTRDIFF_MAX__ etc., make sure
   <limits.h> and (if available) <stdint.h> are included
   so that the code can choose integer types of a good width.  */

#ifndef __PTRDIFF_MAX__
# include <limits.h> /* INFRINGES ON USER NAME SPACE */
# if defined __STDC_VERSION__ && 199901 <= __STDC_VERSION__
#  include <stdint.h> /* INFRINGES ON USER NAME SPACE */
#  define YY_STDINT_H
# endif
#endif

/* Narrow types that promote to a signed type and that can represent a
   signed or unsigned integer of at least N bits.  In tables they can
   save space and decrease cache pressure.  Promoting to a signed type
   helps avoid bugs in integer arithmetic.  */

#ifdef __INT_LEAST8_MAX__
typedef __INT_LEAST8_TYPE__ yytype_int8;
#elif defined YY_STDINT_H
typedef int_least8_t yytype_int8;
#else
typedef signed char yytype_int8;
#endif

#ifdef __INT_LEAST16_MAX__
typedef __INT_LEAST16_TYPE__ yytype_int16;
#elif defined YY_STDINT_H
typedef int_least16_t yytype_int16;
#else
typedef short yytype_int16;
#endif

/* Work around bug in HP-UX 11.23, which defines these macros
   incorrectly for preprocessor constants.  This workaround can likely
   be removed in 2023, as HPE has promised support for HP-UX 11.23
   (aka HP-UX 11i v2) only through the end of 2022; see Table 2 of
   <https://h20195.www2.hpe.com/V2/getpdf.aspx/4AA4-7673ENW.pdf>.  */
#ifdef __hpux
# undef UINT_LEAST8_MAX
# undef UINT_LEAST16_MAX
# define UINT_LEAST8_MAX 255
# define UINT_LEAST16_MAX 65535
#endif

#if defined __UINT_LEAST8_MAX__ && __UINT_LEAST8_MAX__ <= __INT_MAX__
typedef __UINT_LEAST8_TYPE__ yytype_uint8;
#elif (!defined __UINT_LEAST8_MAX__ && defined YY_STDINT_H \
       && UINT_LEAST8_MAX <= INT_MAX)
typedef uint_least8_t yytype_uint8;
#elif !defined __UINT_LEAST8_MAX__ && UCHAR_MAX <= INT_MAX
typedef unsigned char yytype_uint8;
#else
typedef short yytype_uint8;
#endif

#if defined __UINT_LEAST16_MAX__ && __UINT_LEAST16_MAX__ <= __INT_MAX__
typedef __UINT_LEAST16_TYPE__ yytype_uint16;
#elif (!defined __UINT_LEAST16_MAX__ && defined YY_STDINT_H \
       && UINT_LEAST16_MAX <= INT_MAX)
typedef uint_least16_t yytype_uint16;
#elif !defined __UINT_LEAST16_MAX__ && USHRT_MAX <= INT_MAX
typedef unsigned short yytype_uint16;
#else
typedef int yytype_uint16;
#endif

#ifndef YYPTRDIFF_T
# if defined __PTRDIFF_TYPE__ && defined __PTRDIFF_MAX__
#  define YYPTRDIFF_T __PTRDIFF_TYPE__
#  define YYPTRDIFF_MAXIMUM __PTRDIFF_MAX__
# elif defined PTRDIFF_MAX
#  ifndef ptrdiff_t
#   include <stddef.h> /* INFRINGES ON USER NAME SPACE */
#  endif
#  define YYPTRDIFF_T ptrdiff_t
#  define YYPTRDIFF_MAXIMUM PTRDIFF_MAX
# else
#  define YYPTRDIFF_T long
#  define YYPTRDIFF_MAXIMUM LONG_MAX
# endif
#endif

#ifndef YYSIZE_T
# ifdef __SIZE_TYPE__
#  define YYSIZE_T __SIZE_TYPE__
# elif defined size_t
#  define YYSIZE_T size_t
# elif defined __STDC_VERSION__ && 199901 <= __STDC_VERSION__
#  include <stddef.h> /* INFRINGES ON USER NAME SPACE */
#  define YYSIZE_T size_t
# else
#  define YYSIZE_T unsigned
# endif
#endif

#define YYSIZE_MAXIMUM                                  \
  YY_CAST (YYPTRDIFF_T,                                 \
           (YYPTRDIFF_MAXIMUM < YY_CAST (YYSIZE_T, -1)  \
            ? YYPTRDIFF_MAXIMUM                         \
            : YY_CAST (YYSIZE_T, -1)))

#define YYSIZEOF(X) YY_CAST (YYPTRDIFF_T, sizeof (X))


/* Stored state numbers (used for stacks). */
typedef yytype_int16 yy_state_t;

/* State numbers in computations.  */
typedef int yy_state_fast_t;

#ifndef YY_
# if defined YYENABLE_NLS && YYENABLE_NLS
#  if ENABLE_NLS
#   include <libintl.h> /* INFRINGES ON USER NAME SPACE */
#   define YY_(Msgid) dgettext ("bison-runtime", Msgid)
#  endif
# endif
# ifndef YY_
#  define YY_(Msgid) Msgid
# endif
#endif


#ifndef YY_ATTRIBUTE_PURE
# if defined __GNUC__ && 2 < __GNUC__ + (96 <= __GNUC_MINOR__)
#  define YY_ATTRIBUTE_PURE __attribute__ ((__pure__))
# else
#  define YY_ATTRIBUTE_PURE
# endif
#endif

#ifndef YY_ATTRIBUTE_UNUSED
# if defined __GNUC__ && 2 < __GNUC__ + (7 <= __GNUC_MINOR__)
#  define YY_ATTRIBUTE_UNUSED __attribute__ ((__unused__))
# else
#  define YY_ATTRIBUTE_UNUSED
# endif
#endif

/* Suppress unused-variable warnings by "using" E.  */
#if ! defined lint || defined __GNUC__
# define YY_USE(E) ((void) (E))
#else
# define YY_USE(E) /* empty */
#endif

/* Suppress an incorrect diagnostic about yylval being uninitialized.  */
#if defined __GNUC__ && ! defined __ICC && 406 <= __GNUC__ * 100 + __GNUC_MINOR__
# if __GNUC__ * 100 + __GNUC_MINOR__ < 407
#  define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN                           \
    _Pragma ("GCC diagnostic push")                                     \
    _Pragma ("GCC diagnostic ignored \"-Wuninitialized\"")
# else
#  define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN                           \
    _Pragma ("GCC diagnostic push")                                     \
    _Pragma ("GCC diagnostic ignored \"-Wuninitialized\"")              \
    _Pragma ("GCC diagnostic ignored \"-Wmaybe-uninitialized\"")
# endif
# define YY_IGNORE_MAYBE_UNINITIALIZED_END      \
    _Pragma ("GCC diagnostic pop")
#else
# define YY_INITIAL_VALUE(Value) Value
#endif
#ifndef YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
# define YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
# define YY_IGNORE_MAYBE_UNINITIALIZED_END
#endif
#ifndef YY_INITIAL_VALUE
# define YY_INITIAL_VALUE(Value) /* Nothing. */
#endif

#if defined __cplusplus && defined __GNUC__ && ! defined __ICC && 6 <= __GNUC__
# define YY_IGNORE_USELESS_CAST_BEGIN                          \
    _Pragma ("GCC diagnostic push")                            \
    _Pragma ("GCC diagnostic ignored \"-Wuseless-cast\"")
# define YY_IGNORE_USELESS_CAST_END            \
    _Pragma ("GCC diagnostic pop")
#endif
#ifndef YY_IGNORE_USELESS_CAST_BEGIN
# define YY_IGNORE_USELESS_CAST_BEGIN
# define YY_IGNORE_USELESS_CAST_END
#endif


#define YY_ASSERT(E) ((void) (0 && (E)))

#if 1

/* The parser invokes alloca or malloc; define the necessary symbols.  */

# ifdef YYSTACK_USE_ALLOCA
#  if YYSTACK_USE_ALLOCA
#   ifdef __GNUC__
#    define YYSTACK_ALLOC __builtin_alloca
#   elif defined __BUILTIN_VA_ARG_INCR
#    include <alloca.h> /* INFRINGES ON USER NAME SPACE */
#   elif defined _AIX
#    define YYSTACK_ALLOC __alloca
#   elif defined _MSC_VER
#    include <malloc.h> /* INFRINGES ON USER NAME SPACE */
#    define alloca _alloca
#   else
#    define YYSTACK_ALLOC alloca
#    if ! defined _ALLOCA_H && ! defined EXIT_SUCCESS
#     include <stdlib.h> /* INFRINGES ON USER NAME SPACE */
      /* Use EXIT_SUCCESS as a witness for stdlib.h.  */
#     ifndef EXIT_SUCCESS
#      define EXIT_SUCCESS 0
#     endif
#    endif
#   endif
#  endif
# endif

# ifdef YYSTACK_ALLOC
   /* Pacify GCC's 'empty if-body' warning.  */
#  define YYSTACK_FREE(Ptr) do { /* empty */; } while (0)
#  ifndef YYSTACK_ALLOC_MAXIMUM
    /* The OS might guarantee only one guard page at the bottom of the stack,
       and a page size can be as small as 4096 bytes.  So we cannot safely
       invoke alloca (N) if N exceeds 4096.  Use a slightly smaller number
       to allow for a few compiler-allocated temporary stack slots.  */
#   define YYSTACK_ALLOC_MAXIMUM 4032 /* reasonable circa 2006 */
#  endif
# else
#  define YYSTACK_ALLOC YYMALLOC
#  define YYSTACK_FREE YYFREE
#  ifndef YYSTACK_ALLOC_MAXIMUM
#   define YYSTACK_ALLOC_MAXIMUM YYSIZE_MAXIMUM
#  endif
#  if (defined __cplusplus && ! defined EXIT_SUCCESS \
       && ! ((defined YYMALLOC || defined malloc) \
             && (defined YYFREE || defined free)))
#   include <stdlib.h> /* INFRINGES ON USER NAME SPACE */
#   ifndef EXIT_SUCCESS
#    define EXIT_SUCCESS 0
#   endif
#  endif
#  ifndef YYMALLOC
#   define YYMALLOC malloc
#   if ! defined malloc && ! defined EXIT_SUCCESS
void *malloc (YYSIZE_T); /* INFRINGES ON USER NAME SPACE */
#   endif
#  endif
#  ifndef YYFREE
#   define YYFREE free
#   if ! defined free && ! defined EXIT_SUCCESS
void free (void *); /* INFRINGES ON USER NAME SPACE */
#   endif
#  endif
# endif
#endif /* 1 */

#if (! defined yyoverflow \
     && (! defined __cplusplus \
         || (defined YYSTYPE_IS_TRIVIAL && YYSTYPE_IS_TRIVIAL)))

/* A type that is properly aligned for any stack member.  */
union yyalloc
{
  yy_state_t yyss_alloc;
  YYSTYPE yyvs_alloc;
};

/* The size of the maximum gap between one aligned stack and the next.  */
# define YYSTACK_GAP_MAXIMUM (YYSIZEOF (union yyalloc) - 1)

/* The size of an array large to enough to hold all stacks, each with
   N elements.  */
# define YYSTACK_BYTES(N) \
     ((N) * (YYSIZEOF (yy_state_t) + YYSIZEOF (YYSTYPE)) \
      + YYSTACK_GAP_MAXIMUM)

# define YYCOPY_NEEDED 1

/* Relocate STACK from its old location to the new one.  The
   local variables YYSIZE and YYSTACKSIZE give the old and new number of
   elements in the stack, and YYPTR gives the new location of the
   stack.  Advance YYPTR to a properly aligned location for the next
   stack.  */
# define YYSTACK_RELOCATE(Stack_alloc, Stack)                           \
    do                                                                  \
      {                                                                 \
        YYPTRDIFF_T yynewbytes;                                         \
        YYCOPY (&yyptr->Stack_alloc, Stack, yysize);                    \
        Stack = &yyptr->Stack_alloc;                                    \
        yynewbytes = yystacksize * YYSIZEOF (*Stack) + YYSTACK_GAP_MAXIMUM; \
        yyptr += yynewbytes / YYSIZEOF (*yyptr);                        \
      }                                                                 \
    while (0)

#endif

#if defined YYCOPY_NEEDED && YYCOPY_NEEDED
/* Copy COUNT objects from SRC to DST.  The source and destination do
   not overlap.  */
# ifndef YYCOPY
#  if defined __GNUC__ && 1 < __GNUC__
#   define YYCOPY(Dst, Src, Count) \
      __builtin_memcpy (Dst, Src, YY_CAST (YYSIZE_T, (Count)) * sizeof (*(Src)))
#  else
#   define YYCOPY(Dst, Src, Count)              \
      do                                        \
        {                                       \
          YYPTRDIFF_T yyi;                      \
          for (yyi = 0; yyi < (Count); yyi++)   \
            (Dst)[yyi] = (Src)[yyi];            \
        }                                       \
      while (0)
#  endif
# endif
#endif /* !YYCOPY_NEEDED */

/* YYFINAL -- State number of the termination state.  */
#define YYFINAL  16
/* YYLAST -- Last index in YYTABLE.  */
#define YYLAST   859

/* YYNTOKENS -- Number of terminals.  */
#define YYNTOKENS  66
/* YYNNTS -- Number of nonterminals.  */
#define YYNNTS  80
/* YYNRULES -- Number of rules.  */
#define YYNRULES  213
/* YYNSTATES -- Number of states.  */
#define YYNSTATES  372

/* YYMAXUTOK -- Last valid token kind.  */
#define YYMAXUTOK   257


/* YYTRANSLATE(TOKEN-NUM) -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex, with out-of-bounds checking.  */
#define YYTRANSLATE(YYX)                                \
  (0 <= (YYX) && (YYX) <= YYMAXUTOK                     \
   ? YY_CAST (yysymbol_kind_t, yytranslate[YYX])        \
   : YYSYMBOL_YYUNDEF)

/* YYTRANSLATE[TOKEN-NUM] -- Symbol number corresponding to TOKEN-NUM
   as returned by yylex.  */
static const yytype_int8 yytranslate[] =
{
       0,     4,     5,     6,     7,     8,     9,    10,    11,     2,
      12,    13,    14,    15,    16,     2,     2,     2,     2,     2,
      17,    18,    19,    20,    21,    22,    23,    24,    25,    26,
      27,    28,    29,    30,    31,    32,    33,    34,    35,     2,
      36,    37,    38,    39,    40,    41,    42,    43,    44,    45,
      46,    47,    48,    49,    50,    51,     2,     2,     2,     2,
      52,    53,    54,    55,    56,    57,     2,     2,     2,     2,
      58,    59,    60,    61,    62,    63,    64,    65,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     3,     1,     2
};

#if YYDEBUG
/* YYRLINE[YYN] -- Source line where rule number YYN was defined.  */
static const yytype_int16 yyrline[] =
{
       0,   266,   266,   268,   270,   274,   280,   281,   282,   288,
     300,   303,   311,   312,   313,   314,   315,   318,   319,   322,
     323,   326,   327,   328,   329,   330,   331,   332,   333,   336,
     351,   361,   364,   370,   375,   380,   385,   390,   395,   400,
     405,   410,   415,   416,   417,   418,   425,   427,   429,   433,
     437,   441,   445,   447,   451,   453,   457,   463,   465,   469,
     481,   484,   490,   496,   497,   500,   502,   506,   507,   508,
     509,   510,   525,   527,   545,   547,   569,   575,   577,   579,
     582,   586,   590,   592,   596,   598,   602,   606,   608,   612,
     614,   618,   622,   623,   626,   628,   630,   632,   634,   638,
     639,   642,   643,   647,   648,   651,   653,   657,   661,   662,
     665,   668,   670,   674,   676,   678,   682,   684,   688,   692,
     696,   697,   698,   699,   702,   703,   706,   708,   710,   712,
     714,   716,   718,   720,   722,   724,   726,   730,   731,   734,
     735,   736,   737,   738,   750,   762,   764,   767,   769,   771,
     773,   775,   777,   781,   783,   785,   787,   789,   791,   793,
     795,   797,   801,   807,   809,   811,   815,   817,   821,   825,
     827,   831,   835,   837,   839,   841,   845,   847,   849,   851,
     855,   861,   863,   865,   869,   871,   878,   884,   896,   898,
     910,   912,   916,   918,   927,   940,   941,   950,  1010,  1011,
    1014,  1015,  1016,  1019,  1022,  1023,  1026,  1027,  1030,  1031,
    1034,  1037,  1038,  1041
};
#endif

/** Accessing symbol of state STATE.  */
#define YY_ACCESSING_SYMBOL(State) YY_CAST (yysymbol_kind_t, yystos[State])

#if 1
/* The user-facing name of the symbol whose (internal) number is
   YYSYMBOL.  No bounds checking.  */
static const char *yysymbol_name (yysymbol_kind_t yysymbol) YY_ATTRIBUTE_UNUSED;

static const char *
yysymbol_name (yysymbol_kind_t yysymbol)
{
  static const char *const yy_sname[] =
  {
  "end of file", "error", "invalid token", "invalid token", "xkb_keymap",
  "xkb_keycodes", "xkb_types", "xkb_symbols", "xkb_compatibility",
  "xkb_geometry", "xkb_semantics", "xkb_layout", "include", "override",
  "augment", "replace", "alternate", "virtual_modifiers", "type",
  "interpret", "action", "key", "alias", "group", "modifier_map",
  "indicator", "shape", "keys", "row", "section", "overlay", "text",
  "outline", "solid", "logo", "virtual", "=", "+", "-", "/", "*", "{", "}",
  "(", ")", "[", "]", ".", ",", ";", "!", "~", "string literal",
  "decimal digit", "integer literal", "float literal", "identifier",
  "key name", "partial", "default", "hidden", "alphanumeric_keys",
  "modifier_keys", "keypad_keys", "function_keys", "alternate_group",
  "$accept", "XkbFile", "XkbCompositeMap", "XkbCompositeType",
  "XkbMapConfigList", "XkbMapConfig", "FileType", "OptFlags", "Flags",
  "Flag", "DeclList", "Decl", "VarDecl", "KeyNameDecl", "KeyAliasDecl",
  "VModDecl", "VModDefList", "VModDef", "InterpretDecl", "InterpretMatch",
  "VarDeclList", "KeyTypeDecl", "SymbolsDecl", "OptSymbolsBody",
  "SymbolsBody", "SymbolsVarDecl", "MultiKeySymOrActionList",
  "NoSymbolOrActionList", "GroupCompatDecl", "ModMapDecl",
  "KeyOrKeySymList", "KeyOrKeySym", "LedMapDecl", "LedNameDecl",
  "ShapeDecl", "SectionDecl", "SectionBody", "SectionBodyItem", "RowBody",
  "RowBodyItem", "Keys", "Key", "OverlayDecl", "OverlayKeyList",
  "OverlayKey", "OutlineList", "OutlineInList", "CoordList", "Coord",
  "DoodadDecl", "DoodadType", "FieldSpec", "Element", "OptMergeMode",
  "MergeMode", "ExprList", "Expr", "Term", "MultiActionList", "ActionList",
  "NonEmptyActions", "Actions", "Action", "Lhs", "Terminal",
  "MultiKeySymList", "KeySymList", "NonEmptyKeySyms", "KeySyms", "KeySym",
  "KeySymLit", "SignedNumber", "Number", "Float", "Integer", "KeyCode",
  "Ident", "String", "OptMapName", "MapName", YY_NULLPTR
  };
  return yy_sname[yysymbol];
}
#endif

#define YYPACT_NINF (-273)

#define yypact_value_is_default(Yyn) \
  ((Yyn) == YYPACT_NINF)

#define YYTABLE_NINF (-209)

#define yytable_value_is_error(Yyn) \
  0

/* YYPACT[STATE-NUM] -- Index in YYTABLE of the portion describing
   STATE-NUM.  */
static const yytype_int16 yypact[] =
{
     180,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,
      64,  -273,  -273,   471,   478,  -273,  -273,  -273,  -273,  -273,
    -273,  -273,  -273,  -273,  -273,    27,    27,  -273,  -273,    63,
    -273,    86,  -273,  -273,   192,     5,    81,  -273,   371,  -273,
    -273,  -273,  -273,  -273,   109,  -273,   266,    93,  -273,  -273,
       6,   138,    88,  -273,   163,   168,    -5,     6,   -29,   138,
    -273,   138,   160,  -273,  -273,  -273,   203,     6,  -273,   196,
    -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,
    -273,  -273,  -273,  -273,  -273,   138,    66,  -273,   200,   214,
    -273,    87,  -273,   241,  -273,   239,  -273,  -273,  -273,  -273,
    -273,   252,   259,  -273,   278,   270,  -273,  -273,   285,   283,
     292,   312,   317,   320,    -5,   318,   155,   329,   331,   800,
     331,  -273,     6,  -273,   331,  -273,  -273,   331,   174,   314,
     331,    -2,   331,  -273,    18,   415,   337,  -273,  -273,  -273,
     340,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,  -273,
    -273,   331,   331,   756,   331,   331,   331,  -273,  -273,   286,
     265,  -273,  -273,  -273,   359,  -273,  -273,  -273,  -273,  -273,
     358,   178,  -273,   380,   598,   613,   380,   432,     6,   363,
     352,  -273,  -273,   376,    48,   364,   221,  -273,    53,  -273,
    -273,   298,   655,   386,   128,    84,  -273,   112,  -273,   401,
     138,   413,   138,  -273,  -273,    11,  -273,  -273,  -273,   331,
    -273,   670,  -273,  -273,  -273,  -273,   399,   120,  -273,   563,
    -273,  -273,   331,   331,   331,   331,   331,  -273,   331,   331,
    -273,   410,  -273,   417,   419,   474,  -273,   421,    52,   189,
    -273,  -273,   201,  -273,  -273,  -273,   418,   174,   289,  -273,
    -273,   420,    -2,  -273,   423,   143,   133,  -273,  -273,  -273,
     422,  -273,   434,    47,   438,   386,   373,   712,   427,   440,
    -273,   326,   441,   331,  -273,   800,  -273,    70,   380,   382,
     382,  -273,  -273,   380,   369,  -273,  -273,  -273,  -273,   162,
    -273,  -273,   494,  -273,   776,  -273,    56,  -273,  -273,  -273,
     380,  -273,  -273,  -273,  -273,  -273,   128,  -273,  -273,  -273,
    -273,   727,   380,   455,  -273,   556,  -273,   444,  -273,  -273,
    -273,   119,  -273,  -273,   331,  -273,  -273,    96,   536,   218,
     230,  -273,  -273,     4,  -273,  -273,  -273,   458,   165,   -35,
     459,  -273,   473,   220,  -273,  -273,   380,  -273,  -273,  -273,
    -273,  -273,  -273,  -273,  -273,   331,  -273,   225,  -273,  -273,
     464,   475,   444,   233,   480,   -35,  -273,  -273,  -273,  -273,
    -273,  -273
};

/* YYDEFACT[STATE-NUM] -- Default reduction number in state STATE-NUM.
   Performed when YYTABLE does not specify something else to do.  Zero
   means the default is an error.  */
static const yytype_uint8 yydefact[] =
{
      18,     4,    21,    22,    23,    24,    25,    26,    27,    28,
       0,     2,     3,     0,    17,    20,     1,     6,    12,    13,
      15,    14,    16,     7,     8,   212,   212,    19,   213,     0,
     211,     0,    10,    31,    18,   138,     0,     9,     0,   139,
     141,   140,   142,   143,     0,    29,     0,   137,     5,    11,
       0,   128,   127,   126,   129,     0,   130,   131,   132,   133,
     134,   135,   136,   121,   122,   123,     0,     0,   208,     0,
     209,    32,    34,    35,    30,    33,    36,    37,    39,    38,
      40,    41,    42,    43,    44,     0,   172,   125,     0,   124,
      45,     0,    53,    54,   210,     0,   195,   193,   196,   197,
     194,     0,    58,   192,     0,     0,   205,   204,     0,     0,
       0,     0,     0,     0,     0,     0,     0,     0,     0,     0,
       0,    47,     0,    51,     0,    60,    60,     0,    64,     0,
       0,     0,     0,    60,     0,     0,     0,    48,   207,   206,
       0,    60,   128,   127,   129,   130,   131,   132,   133,   135,
     136,     0,     0,     0,     0,     0,     0,   203,   179,   172,
       0,   152,   169,   159,   157,   160,   178,   177,   124,   176,
     173,     0,    52,    55,     0,     0,    57,    79,     0,     0,
      63,    66,    71,     0,   124,     0,     0,    84,     0,    83,
      85,     0,     0,     0,     0,     0,   112,     0,   117,     0,
     132,   134,     0,    95,    97,     0,    93,    98,    96,     0,
      49,     0,   154,   157,   153,   170,     0,     0,   167,     0,
     155,   156,   146,     0,     0,     0,     0,   174,     0,     0,
      46,     0,    59,     0,   195,     0,   189,   194,     0,     0,
     165,   164,     0,   183,   182,    70,     0,     0,     0,    50,
      80,     0,     0,    87,     0,     0,     0,   201,   202,   200,
       0,   199,     0,     0,     0,     0,     0,     0,     0,     0,
      92,     0,     0,   146,   168,     0,   161,     0,   145,   148,
     149,   147,   150,   151,     0,    61,    56,    78,   187,     0,
     186,    76,     0,    74,     0,    72,     0,    62,    65,    68,
      67,    81,    82,    86,   113,   198,     0,    89,   111,    90,
     116,     0,   115,     0,   102,     0,   100,     0,    91,    88,
     119,     0,   166,   158,     0,   175,   188,     0,     0,     0,
       0,   163,   162,     0,   190,   181,   180,     0,     0,     0,
       0,    99,     0,     0,   109,   171,   144,   185,   184,    77,
      75,    73,   191,   118,   114,   146,   105,     0,   104,    94,
       0,     0,     0,     0,     0,     0,   110,   107,   108,   106,
     101,   103
};

/* YYPGOTO[NTERM-NUM].  */
static const yytype_int16 yypgoto[] =
{
    -273,  -273,  -273,  -273,  -273,   497,  -273,   498,  -273,   520,
    -273,  -273,   -44,  -273,  -273,  -273,  -273,   429,  -273,  -273,
     -57,  -273,  -273,  -273,  -273,   297,   301,  -273,  -273,  -273,
    -273,   293,   506,  -273,  -273,  -273,  -273,   353,  -273,   248,
    -273,   204,  -273,  -273,   206,  -273,   303,  -190,   305,   525,
    -273,   -46,  -273,  -273,  -273,  -272,   -52,   355,   280,  -273,
    -169,   279,  -170,   -36,  -273,   294,  -273,   295,  -273,   541,
    -149,   288,   341,  -273,   -43,  -273,   -41,   -47,   570,  -273
};

/* YYDEFGOTO[NTERM-NUM].  */
static const yytype_int16 yydefgoto[] =
{
       0,    10,    11,    25,    34,    12,    26,    13,    14,    15,
      35,    45,   232,    72,    73,    74,    91,    92,    75,   101,
     174,    76,    77,   179,   180,   181,   182,   238,    78,    79,
     188,   189,   204,    81,    82,    83,   205,   206,   315,   316,
     357,   358,   207,   343,   344,   195,   196,   197,   198,   208,
      85,   159,    87,    46,    47,   277,   278,   161,   239,   217,
     162,   163,   218,   164,   165,   242,   289,   243,   335,   190,
     103,   260,   261,   166,   167,   140,   168,   169,    29,    30
};

/* YYTABLE[YYPACT[STATE-NUM]] -- What to do in state STATE-NUM.  If
   positive, shift that token.  If negative, reduce the rule whose
   number is the opposite.  If YYTABLE_NINF, syntax error.  */
static const yytype_int16 yytable[] =
{
      86,   321,    71,   255,    95,    89,   355,   241,   240,    93,
      88,   111,   112,   108,   113,   110,   109,    39,    40,    41,
      42,    43,   356,    94,   106,   107,   115,    96,   244,   142,
     143,    53,   144,    96,   145,   146,   200,   148,   117,   201,
     149,   202,    62,    63,    64,    65,   352,    44,   106,   107,
      97,    98,    99,   269,   100,   187,   288,    98,    99,   193,
     100,    67,    68,   194,    16,    70,   160,    68,   171,   175,
      70,   136,   173,   170,    68,   176,   192,    70,   186,    28,
     191,    93,    86,   363,   211,    96,   290,   184,   193,    86,
     -69,   203,   183,   199,    89,   251,   -69,   333,   291,    88,
     292,   252,   219,    68,    32,   322,    70,   216,   236,    98,
      99,   118,   100,   119,   323,   213,   213,    96,   324,   213,
     213,   338,   241,   240,   332,    96,   262,    33,    86,    86,
      48,   216,   263,    89,    89,   122,   123,   245,    88,    88,
      97,    98,    99,   244,   100,    90,    86,   336,   347,    98,
      99,    89,   100,   111,   264,   268,    88,   271,    49,    86,
     265,   203,   274,   345,    89,    86,   256,   324,   275,    88,
      89,   279,   280,   281,   282,    88,   283,   284,   348,   290,
       1,   257,   258,   259,   290,   304,   257,   258,   259,   216,
      94,   265,   142,   143,    53,   144,   300,   145,   146,   147,
     148,    86,    60,   149,   326,   150,   184,   354,   138,   139,
     327,   183,  -120,   265,   312,   223,   224,   225,   226,   177,
     104,    86,   199,   314,   178,   105,    89,   230,   114,   216,
      68,    88,   116,    70,    36,   293,   120,   294,     2,     3,
       4,     5,     6,     7,     8,     9,   216,   295,   216,   296,
       2,     3,     4,     5,     6,     7,     8,     9,   223,   224,
     225,   226,   361,   121,   350,   216,   294,   364,   362,    86,
     250,   314,   346,   365,    89,   369,   351,   124,   296,    88,
     125,   324,   216,    50,    51,    52,    53,    54,    55,    56,
      57,    58,    59,   126,    60,    61,   127,    62,    63,    64,
      65,    66,   223,   224,   225,   226,   129,   142,   143,    53,
     144,   227,   145,   146,   147,   148,    67,    60,   149,   128,
     150,   130,    68,    69,   131,    70,   151,   152,   132,   222,
     153,   118,   154,   119,   177,   223,   224,   225,   226,   155,
     156,    94,   106,   107,   157,    68,   158,   253,    70,   142,
     143,    53,   144,   133,   145,   146,   147,   148,   134,    60,
     149,   135,   150,   223,   224,   225,   226,   137,   151,   152,
     141,   185,   153,   209,   154,   319,    18,    19,    20,    21,
      22,   155,   156,    94,   106,   107,   157,    68,   158,   210,
      70,   142,   143,    53,   144,   228,   145,   146,   147,   148,
     247,    60,   149,   229,   150,   246,   223,   224,   225,   226,
     151,   152,   248,   249,   311,   325,   154,   223,   224,   225,
     226,   225,   226,   155,   156,    94,   106,   107,   157,    68,
     158,   194,    70,   142,   143,    53,   144,   266,   145,   146,
     200,   148,   273,   201,   149,   202,    62,    63,    64,    65,
     142,   143,    53,   144,   267,   145,   146,   147,   148,   285,
      60,   234,  -135,   150,  -208,    67,   286,   297,   317,   301,
     306,    68,   303,   235,    70,    17,    18,    19,    20,    21,
      22,    23,    24,   307,   236,    98,    99,   309,   237,   318,
     320,    70,   142,   143,    53,   144,   339,   145,   146,   147,
     148,   342,    60,   234,   353,   150,   212,   214,   359,   360,
     220,   221,   142,   143,    53,   144,   287,   145,   146,   147,
     148,   366,    60,   234,   367,   150,   288,    98,    99,   370,
     237,    37,    38,    70,    27,   328,     2,     3,     4,     5,
       6,     7,     8,     9,   298,   302,   236,    98,    99,   299,
     237,   172,    80,    70,   142,   143,    53,   144,   270,   145,
     146,   147,   148,   341,    60,   234,   308,   150,   368,   371,
     310,    84,   329,   331,   142,   143,    53,   144,   349,   145,
     146,   147,   148,   313,    60,   149,   330,   150,   288,    98,
      99,   334,   237,   102,   337,    70,    31,   305,   340,     0,
     223,   224,   225,   226,     0,     0,    67,   276,     0,     0,
       0,     0,    68,     0,     0,    70,   142,   143,    53,   144,
       0,   145,   146,   147,   148,     0,    60,   149,     0,   150,
       0,   142,   143,    53,   144,     0,   145,   146,   147,   148,
     231,    60,   149,     0,   150,     0,     0,     0,    67,     0,
       0,     0,     0,     0,    68,   233,     0,    70,     0,     0,
       0,     0,     0,    67,     0,     0,     0,     0,     0,    68,
       0,     0,    70,   142,   143,    53,   144,     0,   145,   146,
     147,   148,     0,    60,   149,     0,   150,     0,   142,   143,
      53,   144,     0,   145,   146,   147,   148,   254,    60,   149,
       0,   150,     0,     0,     0,    67,     0,     0,     0,     0,
       0,    68,   272,     0,    70,     0,     0,     0,     0,     0,
      67,     0,     0,     0,     0,     0,    68,     0,     0,    70,
     142,   143,    53,   144,     0,   145,   146,   147,   148,   313,
      60,   149,     0,   150,     0,   142,   143,    53,   144,     0,
     145,   146,   147,   148,     0,    60,   149,     0,   150,     0,
       0,     0,    67,     0,     0,     0,     0,     0,    68,   215,
       0,    70,   194,     0,   142,   143,    53,   144,     0,   145,
     146,   147,   148,    68,    60,   149,    70,   150,     0,     0,
       0,     0,     0,     0,   142,   143,    53,   144,   215,   145,
     146,   147,   148,     0,    60,   149,     0,   150,     0,     0,
       0,     0,    68,     0,     0,    70,     0,   153,   142,   143,
      53,   144,     0,   145,   146,   147,   148,     0,    60,   149,
       0,   150,    68,     0,     0,    70,     0,     0,     0,     0,
       0,     0,     0,     0,     0,     0,     0,     0,     0,     0,
       0,     0,     0,     0,     0,     0,    68,     0,     0,    70
};

static const yytype_int16 yycheck[] =
{
      46,   273,    46,   193,    51,    46,    41,   177,   177,    50,
      46,    58,    59,    56,    61,    58,    57,    12,    13,    14,
      15,    16,    57,    52,    53,    54,    67,    29,   177,    18,
      19,    20,    21,    29,    23,    24,    25,    26,    85,    28,
      29,    30,    31,    32,    33,    34,    42,    42,    53,    54,
      52,    53,    54,    42,    56,    57,    52,    53,    54,    41,
      56,    50,    56,    45,     0,    59,   118,    56,   120,   126,
      59,   114,   124,   119,    56,   127,   133,    59,   130,    52,
     132,   122,   128,   355,   141,    29,   235,   128,    41,   135,
      42,   135,   128,   134,   135,    42,    48,    41,    46,   135,
      48,    48,   154,    56,    41,   275,    59,   153,    52,    53,
      54,    45,    56,    47,    44,   151,   152,    29,    48,   155,
     156,   311,   292,   292,   294,    29,    42,    41,   174,   175,
      49,   177,    48,   174,   175,    48,    49,   178,   174,   175,
      52,    53,    54,   292,    56,    52,   192,   296,    52,    53,
      54,   192,    56,   200,    42,   202,   192,   209,    49,   205,
      48,   205,    42,    44,   205,   211,    38,    48,    48,   205,
     211,   223,   224,   225,   226,   211,   228,   229,   327,   328,
       0,    53,    54,    55,   333,    42,    53,    54,    55,   235,
      52,    48,    18,    19,    20,    21,   248,    23,    24,    25,
      26,   247,    28,    29,    42,    31,   247,    42,    53,    54,
      48,   247,    52,    48,   266,    37,    38,    39,    40,    45,
      57,   267,   263,   267,    50,    57,   267,    49,    25,   275,
      56,   267,    36,    59,    42,    46,    36,    48,    58,    59,
      60,    61,    62,    63,    64,    65,   292,    46,   294,    48,
      58,    59,    60,    61,    62,    63,    64,    65,    37,    38,
      39,    40,    42,    49,    46,   311,    48,    42,    48,   315,
      49,   315,   324,    48,   315,    42,    46,    36,    48,   315,
      41,    48,   328,    17,    18,    19,    20,    21,    22,    23,
      24,    25,    26,    41,    28,    29,    37,    31,    32,    33,
      34,    35,    37,    38,    39,    40,    36,    18,    19,    20,
      21,    46,    23,    24,    25,    26,    50,    28,    29,    41,
      31,    36,    56,    57,    41,    59,    37,    38,    36,    43,
      41,    45,    43,    47,    45,    37,    38,    39,    40,    50,
      51,    52,    53,    54,    55,    56,    57,    49,    59,    18,
      19,    20,    21,    41,    23,    24,    25,    26,    41,    28,
      29,    41,    31,    37,    38,    39,    40,    49,    37,    38,
      41,    57,    41,    36,    43,    49,     5,     6,     7,     8,
       9,    50,    51,    52,    53,    54,    55,    56,    57,    49,
      59,    18,    19,    20,    21,    36,    23,    24,    25,    26,
      48,    28,    29,    45,    31,    42,    37,    38,    39,    40,
      37,    38,    36,    49,    41,    46,    43,    37,    38,    39,
      40,    39,    40,    50,    51,    52,    53,    54,    55,    56,
      57,    45,    59,    18,    19,    20,    21,    36,    23,    24,
      25,    26,    43,    28,    29,    30,    31,    32,    33,    34,
      18,    19,    20,    21,    41,    23,    24,    25,    26,    49,
      28,    29,    43,    31,    43,    50,    49,    49,    41,    49,
      48,    56,    49,    41,    59,     4,     5,     6,     7,     8,
       9,    10,    11,    49,    52,    53,    54,    49,    56,    49,
      49,    59,    18,    19,    20,    21,    41,    23,    24,    25,
      26,    57,    28,    29,    46,    31,   151,   152,    49,    36,
     155,   156,    18,    19,    20,    21,    42,    23,    24,    25,
      26,    57,    28,    29,    49,    31,    52,    53,    54,    49,
      56,    34,    34,    59,    14,    41,    58,    59,    60,    61,
      62,    63,    64,    65,   247,   252,    52,    53,    54,   248,
      56,   122,    46,    59,    18,    19,    20,    21,   205,    23,
      24,    25,    26,   315,    28,    29,   263,    31,   362,   365,
     265,    46,   292,   294,    18,    19,    20,    21,    42,    23,
      24,    25,    26,    27,    28,    29,   292,    31,    52,    53,
      54,   296,    56,    52,   306,    59,    26,   256,    42,    -1,
      37,    38,    39,    40,    -1,    -1,    50,    44,    -1,    -1,
      -1,    -1,    56,    -1,    -1,    59,    18,    19,    20,    21,
      -1,    23,    24,    25,    26,    -1,    28,    29,    -1,    31,
      -1,    18,    19,    20,    21,    -1,    23,    24,    25,    26,
      42,    28,    29,    -1,    31,    -1,    -1,    -1,    50,    -1,
      -1,    -1,    -1,    -1,    56,    42,    -1,    59,    -1,    -1,
      -1,    -1,    -1,    50,    -1,    -1,    -1,    -1,    -1,    56,
      -1,    -1,    59,    18,    19,    20,    21,    -1,    23,    24,
      25,    26,    -1,    28,    29,    -1,    31,    -1,    18,    19,
      20,    21,    -1,    23,    24,    25,    26,    42,    28,    29,
      -1,    31,    -1,    -1,    -1,    50,    -1,    -1,    -1,    -1,
      -1,    56,    42,    -1,    59,    -1,    -1,    -1,    -1,    -1,
      50,    -1,    -1,    -1,    -1,    -1,    56,    -1,    -1,    59,
      18,    19,    20,    21,    -1,    23,    24,    25,    26,    27,
      28,    29,    -1,    31,    -1,    18,    19,    20,    21,    -1,
      23,    24,    25,    26,    -1,    28,    29,    -1,    31,    -1,
      -1,    -1,    50,    -1,    -1,    -1,    -1,    -1,    56,    42,
      -1,    59,    45,    -1,    18,    19,    20,    21,    -1,    23,
      24,    25,    26,    56,    28,    29,    59,    31,    -1,    -1,
      -1,    -1,    -1,    -1,    18,    19,    20,    21,    42,    23,
      24,    25,    26,    -1,    28,    29,    -1,    31,    -1,    -1,
      -1,    -1,    56,    -1,    -1,    59,    -1,    41,    18,    19,
      20,    21,    -1,    23,    24,    25,    26,    -1,    28,    29,
      -1,    31,    56,    -1,    -1,    59,    -1,    -1,    -1,    -1,
      -1,    -1,    -1,    -1,    -1,    -1,    -1,    -1,    -1,    -1,
      -1,    -1,    -1,    -1,    -1,    -1,    56,    -1,    -1,    59
};

/* YYSTOS[STATE-NUM] -- The symbol kind of the accessing symbol of
   state STATE-NUM.  */
static const yytype_uint8 yystos[] =
{
       0,     0,    58,    59,    60,    61,    62,    63,    64,    65,
      67,    68,    71,    73,    74,    75,     0,     4,     5,     6,
       7,     8,     9,    10,    11,    69,    72,    75,    52,   144,
     145,   144,    41,    41,    70,    76,    42,    71,    73,    12,
      13,    14,    15,    16,    42,    77,   119,   120,    49,    49,
      17,    18,    19,    20,    21,    22,    23,    24,    25,    26,
      28,    29,    31,    32,    33,    34,    35,    50,    56,    57,
      59,    78,    79,    80,    81,    84,    87,    88,    94,    95,
      98,    99,   100,   101,   115,   116,   117,   118,   129,   142,
      52,    82,    83,   142,    52,   143,    29,    52,    53,    54,
      56,    85,   135,   136,    57,    57,    53,    54,   140,   142,
     140,   143,   143,   143,    25,   142,    36,   143,    45,    47,
      36,    49,    48,    49,    36,    41,    41,    37,    41,    36,
      36,    41,    36,    41,    41,    41,   140,    49,    53,    54,
     141,    41,    18,    19,    21,    23,    24,    25,    26,    29,
      31,    37,    38,    41,    43,    50,    51,    55,    57,   117,
     122,   123,   126,   127,   129,   130,   139,   140,   142,   143,
     117,   122,    83,   122,    86,    86,   122,    45,    50,    89,
      90,    91,    92,   129,   142,    57,   122,    57,    96,    97,
     135,   122,    86,    41,    45,   111,   112,   113,   114,   142,
      25,    28,    30,    78,    98,   102,   103,   108,   115,    36,
      49,    86,   123,   129,   123,    42,   117,   125,   128,   122,
     123,   123,    43,    37,    38,    39,    40,    46,    36,    45,
      49,    42,    78,    42,    29,    41,    52,    56,    93,   124,
     126,   128,   131,   133,   136,   142,    42,    48,    36,    49,
      49,    42,    48,    49,    42,   113,    38,    53,    54,    55,
     137,   138,    42,    48,    42,    48,    36,    41,   143,    42,
     103,   122,    42,    43,    42,    48,    44,   121,   122,   122,
     122,   122,   122,   122,   122,    49,    49,    42,    52,   132,
     136,    46,    48,    46,    48,    46,    48,    49,    91,    92,
     122,    49,    97,    49,    42,   138,    48,    49,   112,    49,
     114,    41,   122,    27,    78,   104,   105,    41,    49,    49,
      49,   121,   128,    44,    48,    46,    42,    48,    41,   124,
     131,   127,   128,    41,   133,   134,   136,   137,   113,    41,
      42,   105,    57,   109,   110,    44,   122,    52,   136,    42,
      46,    46,    42,    46,    42,    41,    57,   106,   107,    49,
      36,    42,    48,   121,    42,    48,    57,    49,   110,    42,
      49,   107
};

/* YYR1[RULE-NUM] -- Symbol kind of the left-hand side of rule RULE-NUM.  */
static const yytype_uint8 yyr1[] =
{
       0,    66,    67,    67,    67,    68,    69,    69,    69,    70,
      70,    71,    72,    72,    72,    72,    72,    73,    73,    74,
      74,    75,    75,    75,    75,    75,    75,    75,    75,    76,
      76,    76,    77,    77,    77,    77,    77,    77,    77,    77,
      77,    77,    77,    77,    77,    77,    78,    78,    78,    79,
      80,    81,    82,    82,    83,    83,    84,    85,    85,    86,
      86,    87,    88,    89,    89,    90,    90,    91,    91,    91,
      91,    91,    92,    92,    92,    92,    92,    93,    93,    93,
      94,    95,    96,    96,    97,    97,    98,    99,    99,   100,
     100,   101,   102,   102,   103,   103,   103,   103,   103,   104,
     104,   105,   105,   106,   106,   107,   107,   108,   109,   109,
     110,   111,   111,   112,   112,   112,   113,   113,   114,   115,
     116,   116,   116,   116,   117,   117,   118,   118,   118,   118,
     118,   118,   118,   118,   118,   118,   118,   119,   119,   120,
     120,   120,   120,   120,   121,   121,   121,   122,   122,   122,
     122,   122,   122,   123,   123,   123,   123,   123,   123,   123,
     123,   123,   124,   124,   124,   124,   125,   125,   126,   127,
     127,   128,   129,   129,   129,   129,   130,   130,   130,   130,
     131,   131,   131,   131,   132,   132,   132,   132,   133,   133,
     134,   134,   135,   135,   136,   136,   136,   136,   137,   137,
     138,   138,   138,   139,   140,   140,   141,   141,   142,   142,
     143,   144,   144,   145
};

/* YYR2[RULE-NUM] -- Number of symbols on the right-hand side of rule RULE-NUM.  */
static const yytype_int8 yyr2[] =
{
       0,     2,     1,     1,     1,     7,     1,     1,     1,     2,
       0,     7,     1,     1,     1,     1,     1,     1,     0,     2,
       1,     1,     1,     1,     1,     1,     1,     1,     1,     2,
       3,     0,     2,     2,     2,     2,     2,     2,     2,     2,
       2,     2,     2,     2,     2,     2,     4,     2,     3,     4,
       5,     3,     3,     1,     1,     3,     6,     3,     1,     2,
       0,     6,     6,     1,     0,     3,     1,     3,     3,     1,
       2,     1,     3,     5,     3,     5,     3,     4,     2,     0,
       5,     6,     3,     1,     1,     1,     6,     5,     6,     6,
       6,     6,     2,     1,     5,     1,     1,     1,     1,     2,
       1,     5,     1,     3,     1,     1,     3,     6,     3,     1,
       3,     3,     1,     3,     5,     3,     3,     1,     5,     6,
       1,     1,     1,     1,     1,     1,     1,     1,     1,     1,
       1,     1,     1,     1,     1,     1,     1,     1,     0,     1,
       1,     1,     1,     1,     3,     1,     0,     3,     3,     3,
       3,     3,     1,     2,     2,     2,     2,     1,     4,     1,
       1,     3,     3,     3,     1,     1,     3,     1,     3,     1,
       2,     4,     1,     3,     4,     6,     1,     1,     1,     1,
       3,     3,     1,     1,     3,     3,     1,     1,     3,     1,
       1,     2,     1,     1,     1,     1,     1,     1,     2,     1,
       1,     1,     1,     1,     1,     1,     1,     1,     1,     1,
       1,     1,     0,     1
};


enum { YYENOMEM = -2 };

#define yyerrok         (yyerrstatus = 0)
#define yyclearin       (yychar = YYEMPTY)

#define YYACCEPT        goto yyacceptlab
#define YYABORT         goto yyabortlab
#define YYERROR         goto yyerrorlab
#define YYNOMEM         goto yyexhaustedlab


#define YYRECOVERING()  (!!yyerrstatus)

#define YYBACKUP(Token, Value)                                    \
  do                                                              \
    if (yychar == YYEMPTY)                                        \
      {                                                           \
        yychar = (Token);                                         \
        yylval = (Value);                                         \
        YYPOPSTACK (yylen);                                       \
        yystate = *yyssp;                                         \
        goto yybackup;                                            \
      }                                                           \
    else                                                          \
      {                                                           \
        yyerror (param, YY_("syntax error: cannot back up")); \
        YYERROR;                                                  \
      }                                                           \
  while (0)

/* Backward compatibility with an undocumented macro.
   Use YYerror or YYUNDEF. */
#define YYERRCODE YYUNDEF


/* Enable debugging if requested.  */
#if YYDEBUG

# ifndef YYFPRINTF
#  include <stdio.h> /* INFRINGES ON USER NAME SPACE */
#  define YYFPRINTF fprintf
# endif

# define YYDPRINTF(Args)                        \
do {                                            \
  if (yydebug)                                  \
    YYFPRINTF Args;                             \
} while (0)




# define YY_SYMBOL_PRINT(Title, Kind, Value, Location)                    \
do {                                                                      \
  if (yydebug)                                                            \
    {                                                                     \
      YYFPRINTF (stderr, "%s ", Title);                                   \
      yy_symbol_print (stderr,                                            \
                  Kind, Value, param); \
      YYFPRINTF (stderr, "\n");                                           \
    }                                                                     \
} while (0)


/*-----------------------------------.
| Print this symbol's value on YYO.  |
`-----------------------------------*/

static void
yy_symbol_value_print (FILE *yyo,
                       yysymbol_kind_t yykind, YYSTYPE const * const yyvaluep, struct parser_param *param)
{
  FILE *yyoutput = yyo;
  YY_USE (yyoutput);
  YY_USE (param);
  if (!yyvaluep)
    return;
  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  YY_USE (yykind);
  YY_IGNORE_MAYBE_UNINITIALIZED_END
}


/*---------------------------.
| Print this symbol on YYO.  |
`---------------------------*/

static void
yy_symbol_print (FILE *yyo,
                 yysymbol_kind_t yykind, YYSTYPE const * const yyvaluep, struct parser_param *param)
{
  YYFPRINTF (yyo, "%s %s (",
             yykind < YYNTOKENS ? "token" : "nterm", yysymbol_name (yykind));

  yy_symbol_value_print (yyo, yykind, yyvaluep, param);
  YYFPRINTF (yyo, ")");
}

/*------------------------------------------------------------------.
| yy_stack_print -- Print the state stack from its BOTTOM up to its |
| TOP (included).                                                   |
`------------------------------------------------------------------*/

static void
yy_stack_print (yy_state_t *yybottom, yy_state_t *yytop)
{
  YYFPRINTF (stderr, "Stack now");
  for (; yybottom <= yytop; yybottom++)
    {
      int yybot = *yybottom;
      YYFPRINTF (stderr, " %d", yybot);
    }
  YYFPRINTF (stderr, "\n");
}

# define YY_STACK_PRINT(Bottom, Top)                            \
do {                                                            \
  if (yydebug)                                                  \
    yy_stack_print ((Bottom), (Top));                           \
} while (0)


/*------------------------------------------------.
| Report that the YYRULE is going to be reduced.  |
`------------------------------------------------*/

static void
yy_reduce_print (yy_state_t *yyssp, YYSTYPE *yyvsp,
                 int yyrule, struct parser_param *param)
{
  int yylno = yyrline[yyrule];
  int yynrhs = yyr2[yyrule];
  int yyi;
  YYFPRINTF (stderr, "Reducing stack by rule %d (line %d):\n",
             yyrule - 1, yylno);
  /* The symbols being reduced.  */
  for (yyi = 0; yyi < yynrhs; yyi++)
    {
      YYFPRINTF (stderr, "   $%d = ", yyi + 1);
      yy_symbol_print (stderr,
                       YY_ACCESSING_SYMBOL (+yyssp[yyi + 1 - yynrhs]),
                       &yyvsp[(yyi + 1) - (yynrhs)], param);
      YYFPRINTF (stderr, "\n");
    }
}

# define YY_REDUCE_PRINT(Rule)          \
do {                                    \
  if (yydebug)                          \
    yy_reduce_print (yyssp, yyvsp, Rule, param); \
} while (0)

/* Nonzero means print parse trace.  It is left uninitialized so that
   multiple parsers can coexist.  */
int yydebug;
#else /* !YYDEBUG */
# define YYDPRINTF(Args) ((void) 0)
# define YY_SYMBOL_PRINT(Title, Kind, Value, Location)
# define YY_STACK_PRINT(Bottom, Top)
# define YY_REDUCE_PRINT(Rule)
#endif /* !YYDEBUG */


/* YYINITDEPTH -- initial size of the parser's stacks.  */
#ifndef YYINITDEPTH
# define YYINITDEPTH 200
#endif

/* YYMAXDEPTH -- maximum size the stacks can grow to (effective only
   if the built-in stack extension method is used).

   Do not make this value too large; the results are undefined if
   YYSTACK_ALLOC_MAXIMUM < YYSTACK_BYTES (YYMAXDEPTH)
   evaluated with infinite-precision integer arithmetic.  */

#ifndef YYMAXDEPTH
# define YYMAXDEPTH 10000
#endif


/* Context of a parse error.  */
typedef struct
{
  yy_state_t *yyssp;
  yysymbol_kind_t yytoken;
} yypcontext_t;

/* Put in YYARG at most YYARGN of the expected tokens given the
   current YYCTX, and return the number of tokens stored in YYARG.  If
   YYARG is null, return the number of expected tokens (guaranteed to
   be less than YYNTOKENS).  Return YYENOMEM on memory exhaustion.
   Return 0 if there are more than YYARGN expected tokens, yet fill
   YYARG up to YYARGN. */
static int
yypcontext_expected_tokens (const yypcontext_t *yyctx,
                            yysymbol_kind_t yyarg[], int yyargn)
{
  /* Actual size of YYARG. */
  int yycount = 0;
  int yyn = yypact[+*yyctx->yyssp];
  if (!yypact_value_is_default (yyn))
    {
      /* Start YYX at -YYN if negative to avoid negative indexes in
         YYCHECK.  In other words, skip the first -YYN actions for
         this state because they are default actions.  */
      int yyxbegin = yyn < 0 ? -yyn : 0;
      /* Stay within bounds of both yycheck and yytname.  */
      int yychecklim = YYLAST - yyn + 1;
      int yyxend = yychecklim < YYNTOKENS ? yychecklim : YYNTOKENS;
      int yyx;
      for (yyx = yyxbegin; yyx < yyxend; ++yyx)
        if (yycheck[yyx + yyn] == yyx && yyx != YYSYMBOL_YYerror
            && !yytable_value_is_error (yytable[yyx + yyn]))
          {
            if (!yyarg)
              ++yycount;
            else if (yycount == yyargn)
              return 0;
            else
              yyarg[yycount++] = YY_CAST (yysymbol_kind_t, yyx);
          }
    }
  if (yyarg && yycount == 0 && 0 < yyargn)
    yyarg[0] = YYSYMBOL_YYEMPTY;
  return yycount;
}




#ifndef yystrlen
# if defined __GLIBC__ && defined _STRING_H
#  define yystrlen(S) (YY_CAST (YYPTRDIFF_T, strlen (S)))
# else
/* Return the length of YYSTR.  */
static YYPTRDIFF_T
yystrlen (const char *yystr)
{
  YYPTRDIFF_T yylen;
  for (yylen = 0; yystr[yylen]; yylen++)
    continue;
  return yylen;
}
# endif
#endif

#ifndef yystpcpy
# if defined __GLIBC__ && defined _STRING_H && defined _GNU_SOURCE
#  define yystpcpy stpcpy
# else
/* Copy YYSRC to YYDEST, returning the address of the terminating '\0' in
   YYDEST.  */
static char *
yystpcpy (char *yydest, const char *yysrc)
{
  char *yyd = yydest;
  const char *yys = yysrc;

  while ((*yyd++ = *yys++) != '\0')
    continue;

  return yyd - 1;
}
# endif
#endif



static int
yy_syntax_error_arguments (const yypcontext_t *yyctx,
                           yysymbol_kind_t yyarg[], int yyargn)
{
  /* Actual size of YYARG. */
  int yycount = 0;
  /* There are many possibilities here to consider:
     - If this state is a consistent state with a default action, then
       the only way this function was invoked is if the default action
       is an error action.  In that case, don't check for expected
       tokens because there are none.
     - The only way there can be no lookahead present (in yychar) is if
       this state is a consistent state with a default action.  Thus,
       detecting the absence of a lookahead is sufficient to determine
       that there is no unexpected or expected token to report.  In that
       case, just report a simple "syntax error".
     - Don't assume there isn't a lookahead just because this state is a
       consistent state with a default action.  There might have been a
       previous inconsistent state, consistent state with a non-default
       action, or user semantic action that manipulated yychar.
     - Of course, the expected token list depends on states to have
       correct lookahead information, and it depends on the parser not
       to perform extra reductions after fetching a lookahead from the
       scanner and before detecting a syntax error.  Thus, state merging
       (from LALR or IELR) and default reductions corrupt the expected
       token list.  However, the list is correct for canonical LR with
       one exception: it will still contain any token that will not be
       accepted due to an error action in a later state.
  */
  if (yyctx->yytoken != YYSYMBOL_YYEMPTY)
    {
      int yyn;
      if (yyarg)
        yyarg[yycount] = yyctx->yytoken;
      ++yycount;
      yyn = yypcontext_expected_tokens (yyctx,
                                        yyarg ? yyarg + 1 : yyarg, yyargn - 1);
      if (yyn == YYENOMEM)
        return YYENOMEM;
      else
        yycount += yyn;
    }
  return yycount;
}

/* Copy into *YYMSG, which is of size *YYMSG_ALLOC, an error message
   about the unexpected token YYTOKEN for the state stack whose top is
   YYSSP.

   Return 0 if *YYMSG was successfully written.  Return -1 if *YYMSG is
   not large enough to hold the message.  In that case, also set
   *YYMSG_ALLOC to the required number of bytes.  Return YYENOMEM if the
   required number of bytes is too large to store.  */
static int
yysyntax_error (YYPTRDIFF_T *yymsg_alloc, char **yymsg,
                const yypcontext_t *yyctx)
{
  enum { YYARGS_MAX = 5 };
  /* Internationalized format string. */
  const char *yyformat = YY_NULLPTR;
  /* Arguments of yyformat: reported tokens (one for the "unexpected",
     one per "expected"). */
  yysymbol_kind_t yyarg[YYARGS_MAX];
  /* Cumulated lengths of YYARG.  */
  YYPTRDIFF_T yysize = 0;

  /* Actual size of YYARG. */
  int yycount = yy_syntax_error_arguments (yyctx, yyarg, YYARGS_MAX);
  if (yycount == YYENOMEM)
    return YYENOMEM;

  switch (yycount)
    {
#define YYCASE_(N, S)                       \
      case N:                               \
        yyformat = S;                       \
        break
    default: /* Avoid compiler warnings. */
      YYCASE_(0, YY_("syntax error"));
      YYCASE_(1, YY_("syntax error, unexpected %s"));
      YYCASE_(2, YY_("syntax error, unexpected %s, expecting %s"));
      YYCASE_(3, YY_("syntax error, unexpected %s, expecting %s or %s"));
      YYCASE_(4, YY_("syntax error, unexpected %s, expecting %s or %s or %s"));
      YYCASE_(5, YY_("syntax error, unexpected %s, expecting %s or %s or %s or %s"));
#undef YYCASE_
    }

  /* Compute error message size.  Don't count the "%s"s, but reserve
     room for the terminator.  */
  yysize = yystrlen (yyformat) - 2 * yycount + 1;
  {
    int yyi;
    for (yyi = 0; yyi < yycount; ++yyi)
      {
        YYPTRDIFF_T yysize1
          = yysize + yystrlen (yysymbol_name (yyarg[yyi]));
        if (yysize <= yysize1 && yysize1 <= YYSTACK_ALLOC_MAXIMUM)
          yysize = yysize1;
        else
          return YYENOMEM;
      }
  }

  if (*yymsg_alloc < yysize)
    {
      *yymsg_alloc = 2 * yysize;
      if (! (yysize <= *yymsg_alloc
             && *yymsg_alloc <= YYSTACK_ALLOC_MAXIMUM))
        *yymsg_alloc = YYSTACK_ALLOC_MAXIMUM;
      return -1;
    }

  /* Avoid sprintf, as that infringes on the user's name space.
     Don't have undefined behavior even if the translation
     produced a string with the wrong number of "%s"s.  */
  {
    char *yyp = *yymsg;
    int yyi = 0;
    while ((*yyp = *yyformat) != '\0')
      if (*yyp == '%' && yyformat[1] == 's' && yyi < yycount)
        {
          yyp = yystpcpy (yyp, yysymbol_name (yyarg[yyi++]));
          yyformat += 2;
        }
      else
        {
          ++yyp;
          ++yyformat;
        }
  }
  return 0;
}


/*-----------------------------------------------.
| Release the memory associated to this symbol.  |
`-----------------------------------------------*/

static void
yydestruct (const char *yymsg,
            yysymbol_kind_t yykind, YYSTYPE *yyvaluep, struct parser_param *param)
{
  YY_USE (yyvaluep);
  YY_USE (param);
  if (!yymsg)
    yymsg = "Deleting";
  YY_SYMBOL_PRINT (yymsg, yykind, yyvaluep, yylocationp);

  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  switch (yykind)
    {
    case YYSYMBOL_STRING: /* "string literal"  */
#line 250 "src/xkbcomp/parser.y"
            { free(((*yyvaluep).str)); }
#line 1604 "parser.c"
        break;

    case YYSYMBOL_XkbFile: /* XkbFile  */
#line 248 "src/xkbcomp/parser.y"
            { if (!param->rtrn) FreeXkbFile(((*yyvaluep).file)); }
#line 1610 "parser.c"
        break;

    case YYSYMBOL_XkbCompositeMap: /* XkbCompositeMap  */
#line 248 "src/xkbcomp/parser.y"
            { if (!param->rtrn) FreeXkbFile(((*yyvaluep).file)); }
#line 1616 "parser.c"
        break;

    case YYSYMBOL_XkbMapConfigList: /* XkbMapConfigList  */
#line 249 "src/xkbcomp/parser.y"
            { FreeXkbFile(((*yyvaluep).fileList).head); }
#line 1622 "parser.c"
        break;

    case YYSYMBOL_XkbMapConfig: /* XkbMapConfig  */
#line 248 "src/xkbcomp/parser.y"
            { if (!param->rtrn) FreeXkbFile(((*yyvaluep).file)); }
#line 1628 "parser.c"
        break;

    case YYSYMBOL_DeclList: /* DeclList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).anyList).head); }
#line 1634 "parser.c"
        break;

    case YYSYMBOL_Decl: /* Decl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).any)); }
#line 1640 "parser.c"
        break;

    case YYSYMBOL_VarDecl: /* VarDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).var)); }
#line 1646 "parser.c"
        break;

    case YYSYMBOL_KeyNameDecl: /* KeyNameDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).keyCode)); }
#line 1652 "parser.c"
        break;

    case YYSYMBOL_KeyAliasDecl: /* KeyAliasDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).keyAlias)); }
#line 1658 "parser.c"
        break;

    case YYSYMBOL_VModDecl: /* VModDecl  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).vmodList).head); }
#line 1664 "parser.c"
        break;

    case YYSYMBOL_VModDefList: /* VModDefList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).vmodList).head); }
#line 1670 "parser.c"
        break;

    case YYSYMBOL_VModDef: /* VModDef  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).vmod)); }
#line 1676 "parser.c"
        break;

    case YYSYMBOL_InterpretDecl: /* InterpretDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).interp)); }
#line 1682 "parser.c"
        break;

    case YYSYMBOL_InterpretMatch: /* InterpretMatch  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).interp)); }
#line 1688 "parser.c"
        break;

    case YYSYMBOL_VarDeclList: /* VarDeclList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).varList).head); }
#line 1694 "parser.c"
        break;

    case YYSYMBOL_KeyTypeDecl: /* KeyTypeDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).keyType)); }
#line 1700 "parser.c"
        break;

    case YYSYMBOL_SymbolsDecl: /* SymbolsDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).syms)); }
#line 1706 "parser.c"
        break;

    case YYSYMBOL_OptSymbolsBody: /* OptSymbolsBody  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).varList).head); }
#line 1712 "parser.c"
        break;

    case YYSYMBOL_SymbolsBody: /* SymbolsBody  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).varList).head); }
#line 1718 "parser.c"
        break;

    case YYSYMBOL_SymbolsVarDecl: /* SymbolsVarDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).var)); }
#line 1724 "parser.c"
        break;

    case YYSYMBOL_MultiKeySymOrActionList: /* MultiKeySymOrActionList  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1730 "parser.c"
        break;

    case YYSYMBOL_GroupCompatDecl: /* GroupCompatDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).groupCompat)); }
#line 1736 "parser.c"
        break;

    case YYSYMBOL_ModMapDecl: /* ModMapDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).modMask)); }
#line 1742 "parser.c"
        break;

    case YYSYMBOL_KeyOrKeySymList: /* KeyOrKeySymList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).exprList).head); }
#line 1748 "parser.c"
        break;

    case YYSYMBOL_KeyOrKeySym: /* KeyOrKeySym  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1754 "parser.c"
        break;

    case YYSYMBOL_LedMapDecl: /* LedMapDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).ledMap)); }
#line 1760 "parser.c"
        break;

    case YYSYMBOL_LedNameDecl: /* LedNameDecl  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).ledName)); }
#line 1766 "parser.c"
        break;

    case YYSYMBOL_CoordList: /* CoordList  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1772 "parser.c"
        break;

    case YYSYMBOL_Coord: /* Coord  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1778 "parser.c"
        break;

    case YYSYMBOL_ExprList: /* ExprList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).exprList).head); }
#line 1784 "parser.c"
        break;

    case YYSYMBOL_Expr: /* Expr  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1790 "parser.c"
        break;

    case YYSYMBOL_Term: /* Term  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1796 "parser.c"
        break;

    case YYSYMBOL_MultiActionList: /* MultiActionList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).exprList).head); }
#line 1802 "parser.c"
        break;

    case YYSYMBOL_ActionList: /* ActionList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).exprList).head); }
#line 1808 "parser.c"
        break;

    case YYSYMBOL_NonEmptyActions: /* NonEmptyActions  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1814 "parser.c"
        break;

    case YYSYMBOL_Actions: /* Actions  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1820 "parser.c"
        break;

    case YYSYMBOL_Action: /* Action  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1826 "parser.c"
        break;

    case YYSYMBOL_Lhs: /* Lhs  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1832 "parser.c"
        break;

    case YYSYMBOL_Terminal: /* Terminal  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1838 "parser.c"
        break;

    case YYSYMBOL_MultiKeySymList: /* MultiKeySymList  */
#line 244 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).exprList).head); }
#line 1844 "parser.c"
        break;

    case YYSYMBOL_KeySymList: /* KeySymList  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1850 "parser.c"
        break;

    case YYSYMBOL_NonEmptyKeySyms: /* NonEmptyKeySyms  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1856 "parser.c"
        break;

    case YYSYMBOL_KeySyms: /* KeySyms  */
#line 241 "src/xkbcomp/parser.y"
            { FreeStmt((ParseCommon *) ((*yyvaluep).expr)); }
#line 1862 "parser.c"
        break;

    case YYSYMBOL_OptMapName: /* OptMapName  */
#line 250 "src/xkbcomp/parser.y"
            { free(((*yyvaluep).str)); }
#line 1868 "parser.c"
        break;

    case YYSYMBOL_MapName: /* MapName  */
#line 250 "src/xkbcomp/parser.y"
            { free(((*yyvaluep).str)); }
#line 1874 "parser.c"
        break;

      default:
        break;
    }
  YY_IGNORE_MAYBE_UNINITIALIZED_END
}






/*----------.
| yyparse.  |
`----------*/

int
yyparse (struct parser_param *param)
{
/* Lookahead token kind.  */
int yychar;


/* The semantic value of the lookahead symbol.  */
/* Default value used for initialization, for pacifying older GCCs
   or non-GCC compilers.  */
YY_INITIAL_VALUE (static YYSTYPE yyval_default;)
YYSTYPE yylval YY_INITIAL_VALUE (= yyval_default);

    /* Number of syntax errors so far.  */
    int yynerrs = 0;

    yy_state_fast_t yystate = 0;
    /* Number of tokens to shift before error messages enabled.  */
    int yyerrstatus = 0;

    /* Refer to the stacks through separate pointers, to allow yyoverflow
       to reallocate them elsewhere.  */

    /* Their size.  */
    YYPTRDIFF_T yystacksize = YYINITDEPTH;

    /* The state stack: array, bottom, top.  */
    yy_state_t yyssa[YYINITDEPTH];
    yy_state_t *yyss = yyssa;
    yy_state_t *yyssp = yyss;

    /* The semantic value stack: array, bottom, top.  */
    YYSTYPE yyvsa[YYINITDEPTH];
    YYSTYPE *yyvs = yyvsa;
    YYSTYPE *yyvsp = yyvs;

  int yyn;
  /* The return value of yyparse.  */
  int yyresult;
  /* Lookahead symbol kind.  */
  yysymbol_kind_t yytoken = YYSYMBOL_YYEMPTY;
  /* The variables used to return semantic value and location from the
     action routines.  */
  YYSTYPE yyval;

  /* Buffer for error messages, and its allocated size.  */
  char yymsgbuf[128];
  char *yymsg = yymsgbuf;
  YYPTRDIFF_T yymsg_alloc = sizeof yymsgbuf;

#define YYPOPSTACK(N)   (yyvsp -= (N), yyssp -= (N))

  /* The number of symbols on the RHS of the reduced rule.
     Keep to zero when no symbol should be popped.  */
  int yylen = 0;

  YYDPRINTF ((stderr, "Starting parse\n"));

  yychar = YYEMPTY; /* Cause a token to be read.  */

  goto yysetstate;


/*------------------------------------------------------------.
| yynewstate -- push a new state, which is found in yystate.  |
`------------------------------------------------------------*/
yynewstate:
  /* In all cases, when you get here, the value and location stacks
     have just been pushed.  So pushing a state here evens the stacks.  */
  yyssp++;


/*--------------------------------------------------------------------.
| yysetstate -- set current state (the top of the stack) to yystate.  |
`--------------------------------------------------------------------*/
yysetstate:
  YYDPRINTF ((stderr, "Entering state %d\n", yystate));
  YY_ASSERT (0 <= yystate && yystate < YYNSTATES);
  YY_IGNORE_USELESS_CAST_BEGIN
  *yyssp = YY_CAST (yy_state_t, yystate);
  YY_IGNORE_USELESS_CAST_END
  YY_STACK_PRINT (yyss, yyssp);

  if (yyss + yystacksize - 1 <= yyssp)
#if !defined yyoverflow && !defined YYSTACK_RELOCATE
    YYNOMEM;
#else
    {
      /* Get the current used size of the three stacks, in elements.  */
      YYPTRDIFF_T yysize = yyssp - yyss + 1;

# if defined yyoverflow
      {
        /* Give user a chance to reallocate the stack.  Use copies of
           these so that the &'s don't force the real ones into
           memory.  */
        yy_state_t *yyss1 = yyss;
        YYSTYPE *yyvs1 = yyvs;

        /* Each stack pointer address is followed by the size of the
           data in use in that stack, in bytes.  This used to be a
           conditional around just the two extra args, but that might
           be undefined if yyoverflow is a macro.  */
        yyoverflow (YY_("memory exhausted"),
                    &yyss1, yysize * YYSIZEOF (*yyssp),
                    &yyvs1, yysize * YYSIZEOF (*yyvsp),
                    &yystacksize);
        yyss = yyss1;
        yyvs = yyvs1;
      }
# else /* defined YYSTACK_RELOCATE */
      /* Extend the stack our own way.  */
      if (YYMAXDEPTH <= yystacksize)
        YYNOMEM;
      yystacksize *= 2;
      if (YYMAXDEPTH < yystacksize)
        yystacksize = YYMAXDEPTH;

      {
        yy_state_t *yyss1 = yyss;
        union yyalloc *yyptr =
          YY_CAST (union yyalloc *,
                   YYSTACK_ALLOC (YY_CAST (YYSIZE_T, YYSTACK_BYTES (yystacksize))));
        if (! yyptr)
          YYNOMEM;
        YYSTACK_RELOCATE (yyss_alloc, yyss);
        YYSTACK_RELOCATE (yyvs_alloc, yyvs);
#  undef YYSTACK_RELOCATE
        if (yyss1 != yyssa)
          YYSTACK_FREE (yyss1);
      }
# endif

      yyssp = yyss + yysize - 1;
      yyvsp = yyvs + yysize - 1;

      YY_IGNORE_USELESS_CAST_BEGIN
      YYDPRINTF ((stderr, "Stack size increased to %ld\n",
                  YY_CAST (long, yystacksize)));
      YY_IGNORE_USELESS_CAST_END

      if (yyss + yystacksize - 1 <= yyssp)
        YYABORT;
    }
#endif /* !defined yyoverflow && !defined YYSTACK_RELOCATE */


  if (yystate == YYFINAL)
    YYACCEPT;

  goto yybackup;


/*-----------.
| yybackup.  |
`-----------*/
yybackup:
  /* Do appropriate processing given the current state.  Read a
     lookahead token if we need one and don't already have one.  */

  /* First try to decide what to do without reference to lookahead token.  */
  yyn = yypact[yystate];
  if (yypact_value_is_default (yyn))
    goto yydefault;

  /* Not known => get a lookahead token if don't already have one.  */

  /* YYCHAR is either empty, or end-of-input, or a valid lookahead.  */
  if (yychar == YYEMPTY)
    {
      YYDPRINTF ((stderr, "Reading a token\n"));
      yychar = yylex (&yylval, param_scanner);
    }

  if (yychar <= END_OF_FILE)
    {
      yychar = END_OF_FILE;
      yytoken = YYSYMBOL_YYEOF;
      YYDPRINTF ((stderr, "Now at end of input.\n"));
    }
  else if (yychar == YYerror)
    {
      /* The scanner already issued an error message, process directly
         to error recovery.  But do not keep the error token as
         lookahead, it is too special and may lead us to an endless
         loop in error recovery. */
      yychar = YYUNDEF;
      yytoken = YYSYMBOL_YYerror;
      goto yyerrlab1;
    }
  else
    {
      yytoken = YYTRANSLATE (yychar);
      YY_SYMBOL_PRINT ("Next token is", yytoken, &yylval, &yylloc);
    }

  /* If the proper action on seeing token YYTOKEN is to reduce or to
     detect an error, take that action.  */
  yyn += yytoken;
  if (yyn < 0 || YYLAST < yyn || yycheck[yyn] != yytoken)
    goto yydefault;
  yyn = yytable[yyn];
  if (yyn <= 0)
    {
      if (yytable_value_is_error (yyn))
        goto yyerrlab;
      yyn = -yyn;
      goto yyreduce;
    }

  /* Count tokens shifted since error; after three, turn off error
     status.  */
  if (yyerrstatus)
    yyerrstatus--;

  /* Shift the lookahead token.  */
  YY_SYMBOL_PRINT ("Shifting", yytoken, &yylval, &yylloc);
  yystate = yyn;
  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  *++yyvsp = yylval;
  YY_IGNORE_MAYBE_UNINITIALIZED_END

  /* Discard the shifted token.  */
  yychar = YYEMPTY;
  goto yynewstate;


/*-----------------------------------------------------------.
| yydefault -- do the default action for the current state.  |
`-----------------------------------------------------------*/
yydefault:
  yyn = yydefact[yystate];
  if (yyn == 0)
    goto yyerrlab;
  goto yyreduce;


/*-----------------------------.
| yyreduce -- do a reduction.  |
`-----------------------------*/
yyreduce:
  /* yyn is the number of a rule to reduce with.  */
  yylen = yyr2[yyn];

  /* If YYLEN is nonzero, implement the default value of the action:
     '$$ = $1'.

     Otherwise, the following line sets YYVAL to garbage.
     This behavior is undocumented and Bison
     users should not rely upon it.  Assigning to YYVAL
     unconditionally makes the parser a bit smaller, and it avoids a
     GCC warning that YYVAL may be used uninitialized.  */
  yyval = yyvsp[1-yylen];


  YY_REDUCE_PRINT (yyn);
  switch (yyn)
    {
  case 2: /* XkbFile: XkbCompositeMap  */
#line 267 "src/xkbcomp/parser.y"
                        { (yyval.file) = param->rtrn = (yyvsp[0].file); param->more_maps = !!param->rtrn; (void) yynerrs; }
#line 2153 "parser.c"
    break;

  case 3: /* XkbFile: XkbMapConfig  */
#line 269 "src/xkbcomp/parser.y"
                        { (yyval.file) = param->rtrn = (yyvsp[0].file); param->more_maps = !!param->rtrn; YYACCEPT; }
#line 2159 "parser.c"
    break;

  case 4: /* XkbFile: "end of file"  */
#line 271 "src/xkbcomp/parser.y"
                        { (yyval.file) = param->rtrn = NULL; param->more_maps = false; }
#line 2165 "parser.c"
    break;

  case 5: /* XkbCompositeMap: OptFlags XkbCompositeType OptMapName "{" XkbMapConfigList "}" ";"  */
#line 277 "src/xkbcomp/parser.y"
                        { (yyval.file) = XkbFileCreate((yyvsp[-5].file_type), (yyvsp[-4].str), (ParseCommon *) (yyvsp[-2].fileList).head, (yyvsp[-6].mapFlags)); }
#line 2171 "parser.c"
    break;

  case 6: /* XkbCompositeType: "xkb_keymap"  */
#line 280 "src/xkbcomp/parser.y"
                                        { (yyval.file_type) = FILE_TYPE_KEYMAP; }
#line 2177 "parser.c"
    break;

  case 7: /* XkbCompositeType: "xkb_semantics"  */
#line 281 "src/xkbcomp/parser.y"
                                        { (yyval.file_type) = FILE_TYPE_KEYMAP; }
#line 2183 "parser.c"
    break;

  case 8: /* XkbCompositeType: "xkb_layout"  */
#line 282 "src/xkbcomp/parser.y"
                                        { (yyval.file_type) = FILE_TYPE_KEYMAP; }
#line 2189 "parser.c"
    break;

  case 9: /* XkbMapConfigList: XkbMapConfigList XkbMapConfig  */
#line 289 "src/xkbcomp/parser.y"
                        {
                            if ((yyvsp[0].file)) {
                                if ((yyvsp[-1].fileList).head) {
                                    (yyval.fileList).head = (yyvsp[-1].fileList).head;
                                    (yyval.fileList).last->common.next = &(yyvsp[0].file)->common;
                                    (yyval.fileList).last = (yyvsp[0].file);
                                } else {
                                    (yyval.fileList).head = (yyval.fileList).last = (yyvsp[0].file);
                                }
                            }
                        }
#line 2205 "parser.c"
    break;

  case 10: /* XkbMapConfigList: %empty  */
#line 300 "src/xkbcomp/parser.y"
                        { (yyval.fileList).head = (yyval.fileList).last = NULL; }
#line 2211 "parser.c"
    break;

  case 11: /* XkbMapConfig: OptFlags FileType OptMapName "{" DeclList "}" ";"  */
#line 306 "src/xkbcomp/parser.y"
                        {
                            (yyval.file) = XkbFileCreate((yyvsp[-5].file_type), (yyvsp[-4].str), (yyvsp[-2].anyList).head, (yyvsp[-6].mapFlags));
                        }
#line 2219 "parser.c"
    break;

  case 12: /* FileType: "xkb_keycodes"  */
#line 311 "src/xkbcomp/parser.y"
                                                { (yyval.file_type) = FILE_TYPE_KEYCODES; }
#line 2225 "parser.c"
    break;

  case 13: /* FileType: "xkb_types"  */
#line 312 "src/xkbcomp/parser.y"
                                                { (yyval.file_type) = FILE_TYPE_TYPES; }
#line 2231 "parser.c"
    break;

  case 14: /* FileType: "xkb_compatibility"  */
#line 313 "src/xkbcomp/parser.y"
                                                { (yyval.file_type) = FILE_TYPE_COMPAT; }
#line 2237 "parser.c"
    break;

  case 15: /* FileType: "xkb_symbols"  */
#line 314 "src/xkbcomp/parser.y"
                                                { (yyval.file_type) = FILE_TYPE_SYMBOLS; }
#line 2243 "parser.c"
    break;

  case 16: /* FileType: "xkb_geometry"  */
#line 315 "src/xkbcomp/parser.y"
                                                { (yyval.file_type) = FILE_TYPE_GEOMETRY; }
#line 2249 "parser.c"
    break;

  case 17: /* OptFlags: Flags  */
#line 318 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = (yyvsp[0].mapFlags); }
#line 2255 "parser.c"
    break;

  case 18: /* OptFlags: %empty  */
#line 319 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = 0; }
#line 2261 "parser.c"
    break;

  case 19: /* Flags: Flags Flag  */
#line 322 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = ((yyvsp[-1].mapFlags) | (yyvsp[0].mapFlags)); }
#line 2267 "parser.c"
    break;

  case 20: /* Flags: Flag  */
#line 323 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = (yyvsp[0].mapFlags); }
#line 2273 "parser.c"
    break;

  case 21: /* Flag: "partial"  */
#line 326 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_IS_PARTIAL; }
#line 2279 "parser.c"
    break;

  case 22: /* Flag: "default"  */
#line 327 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_IS_DEFAULT; }
#line 2285 "parser.c"
    break;

  case 23: /* Flag: "hidden"  */
#line 328 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_IS_HIDDEN; }
#line 2291 "parser.c"
    break;

  case 24: /* Flag: "alphanumeric_keys"  */
#line 329 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_HAS_ALPHANUMERIC; }
#line 2297 "parser.c"
    break;

  case 25: /* Flag: "modifier_keys"  */
#line 330 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_HAS_MODIFIER; }
#line 2303 "parser.c"
    break;

  case 26: /* Flag: "keypad_keys"  */
#line 331 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_HAS_KEYPAD; }
#line 2309 "parser.c"
    break;

  case 27: /* Flag: "function_keys"  */
#line 332 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_HAS_FN; }
#line 2315 "parser.c"
    break;

  case 28: /* Flag: "alternate_group"  */
#line 333 "src/xkbcomp/parser.y"
                                                { (yyval.mapFlags) = MAP_IS_ALTGR; }
#line 2321 "parser.c"
    break;

  case 29: /* DeclList: DeclList Decl  */
#line 337 "src/xkbcomp/parser.y"
                        {
                            if ((yyvsp[0].any)) {
                                if ((yyvsp[-1].anyList).head) {
                                    (yyval.anyList).head = (yyvsp[-1].anyList).head; (yyvsp[-1].anyList).last->next = (yyvsp[0].any); (yyval.anyList).last = (yyvsp[0].any);
                                } else {
                                    (yyval.anyList).head = (yyval.anyList).last = (yyvsp[0].any);
                                }
                            }
                        }
#line 2335 "parser.c"
    break;

  case 30: /* DeclList: DeclList OptMergeMode VModDecl  */
#line 352 "src/xkbcomp/parser.y"
                        {
                            for (VModDef *vmod = (yyvsp[0].vmodList).head; vmod; vmod = (VModDef *) vmod->common.next)
                                vmod->merge = (yyvsp[-1].merge);
                            if ((yyvsp[-2].anyList).head) {
                                (yyval.anyList).head = (yyvsp[-2].anyList).head; (yyvsp[-2].anyList).last->next = &(yyvsp[0].vmodList).head->common; (yyval.anyList).last = &(yyvsp[0].vmodList).last->common;
                            } else {
                                (yyval.anyList).head = &(yyvsp[0].vmodList).head->common; (yyval.anyList).last = &(yyvsp[0].vmodList).last->common;
                            }
                        }
#line 2349 "parser.c"
    break;

  case 31: /* DeclList: %empty  */
#line 361 "src/xkbcomp/parser.y"
                        { (yyval.anyList).head = (yyval.anyList).last = NULL; }
#line 2355 "parser.c"
    break;

  case 32: /* Decl: OptMergeMode VarDecl  */
#line 365 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].var)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].var);
                        }
#line 2364 "parser.c"
    break;

  case 33: /* Decl: OptMergeMode InterpretDecl  */
#line 371 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].interp)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].interp);
                        }
#line 2373 "parser.c"
    break;

  case 34: /* Decl: OptMergeMode KeyNameDecl  */
#line 376 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].keyCode)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].keyCode);
                        }
#line 2382 "parser.c"
    break;

  case 35: /* Decl: OptMergeMode KeyAliasDecl  */
#line 381 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].keyAlias)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].keyAlias);
                        }
#line 2391 "parser.c"
    break;

  case 36: /* Decl: OptMergeMode KeyTypeDecl  */
#line 386 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].keyType)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].keyType);
                        }
#line 2400 "parser.c"
    break;

  case 37: /* Decl: OptMergeMode SymbolsDecl  */
#line 391 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].syms)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].syms);
                        }
#line 2409 "parser.c"
    break;

  case 38: /* Decl: OptMergeMode ModMapDecl  */
#line 396 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].modMask)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].modMask);
                        }
#line 2418 "parser.c"
    break;

  case 39: /* Decl: OptMergeMode GroupCompatDecl  */
#line 401 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].groupCompat)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].groupCompat);
                        }
#line 2427 "parser.c"
    break;

  case 40: /* Decl: OptMergeMode LedMapDecl  */
#line 406 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].ledMap)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].ledMap);
                        }
#line 2436 "parser.c"
    break;

  case 41: /* Decl: OptMergeMode LedNameDecl  */
#line 411 "src/xkbcomp/parser.y"
                        {
                            (yyvsp[0].ledName)->merge = (yyvsp[-1].merge);
                            (yyval.any) = (ParseCommon *) (yyvsp[0].ledName);
                        }
#line 2445 "parser.c"
    break;

  case 42: /* Decl: OptMergeMode ShapeDecl  */
#line 415 "src/xkbcomp/parser.y"
                                                        { (yyval.any) = NULL; }
#line 2451 "parser.c"
    break;

  case 43: /* Decl: OptMergeMode SectionDecl  */
#line 416 "src/xkbcomp/parser.y"
                                                        { (yyval.any) = NULL; }
#line 2457 "parser.c"
    break;

  case 44: /* Decl: OptMergeMode DoodadDecl  */
#line 417 "src/xkbcomp/parser.y"
                                                        { (yyval.any) = NULL; }
#line 2463 "parser.c"
    break;

  case 45: /* Decl: MergeMode "string literal"  */
#line 419 "src/xkbcomp/parser.y"
                        {
                            (yyval.any) = (ParseCommon *) IncludeCreate(param->ctx, (yyvsp[0].str), (yyvsp[-1].merge));
                            free((yyvsp[0].str));
                        }
#line 2472 "parser.c"
    break;

  case 46: /* VarDecl: Lhs "=" Expr ";"  */
#line 426 "src/xkbcomp/parser.y"
                        { (yyval.var) = VarCreate((yyvsp[-3].expr), (yyvsp[-1].expr)); }
#line 2478 "parser.c"
    break;

  case 47: /* VarDecl: Ident ";"  */
#line 428 "src/xkbcomp/parser.y"
                        { (yyval.var) = BoolVarCreate((yyvsp[-1].atom), true); }
#line 2484 "parser.c"
    break;

  case 48: /* VarDecl: "!" Ident ";"  */
#line 430 "src/xkbcomp/parser.y"
                        { (yyval.var) = BoolVarCreate((yyvsp[-1].atom), false); }
#line 2490 "parser.c"
    break;

  case 49: /* KeyNameDecl: "key name" "=" KeyCode ";"  */
#line 434 "src/xkbcomp/parser.y"
                        { (yyval.keyCode) = KeycodeCreate((yyvsp[-3].atom), (yyvsp[-1].num)); }
#line 2496 "parser.c"
    break;

  case 50: /* KeyAliasDecl: "alias" "key name" "=" "key name" ";"  */
#line 438 "src/xkbcomp/parser.y"
                        { (yyval.keyAlias) = KeyAliasCreate((yyvsp[-3].atom), (yyvsp[-1].atom)); }
#line 2502 "parser.c"
    break;

  case 51: /* VModDecl: "virtual_modifiers" VModDefList ";"  */
#line 442 "src/xkbcomp/parser.y"
                        { (yyval.vmodList) = (yyvsp[-1].vmodList); }
#line 2508 "parser.c"
    break;

  case 52: /* VModDefList: VModDefList "," VModDef  */
#line 446 "src/xkbcomp/parser.y"
                        { (yyval.vmodList).head = (yyvsp[-2].vmodList).head; (yyval.vmodList).last->common.next = &(yyvsp[0].vmod)->common; (yyval.vmodList).last = (yyvsp[0].vmod); }
#line 2514 "parser.c"
    break;

  case 53: /* VModDefList: VModDef  */
#line 448 "src/xkbcomp/parser.y"
                        { (yyval.vmodList).head = (yyval.vmodList).last = (yyvsp[0].vmod); }
#line 2520 "parser.c"
    break;

  case 54: /* VModDef: Ident  */
#line 452 "src/xkbcomp/parser.y"
                        { (yyval.vmod) = VModCreate((yyvsp[0].atom), NULL); }
#line 2526 "parser.c"
    break;

  case 55: /* VModDef: Ident "=" Expr  */
#line 454 "src/xkbcomp/parser.y"
                        { (yyval.vmod) = VModCreate((yyvsp[-2].atom), (yyvsp[0].expr)); }
#line 2532 "parser.c"
    break;

  case 56: /* InterpretDecl: "interpret" InterpretMatch "{" VarDeclList "}" ";"  */
#line 460 "src/xkbcomp/parser.y"
                        { (yyvsp[-4].interp)->def = (yyvsp[-2].varList).head; (yyval.interp) = (yyvsp[-4].interp); }
#line 2538 "parser.c"
    break;

  case 57: /* InterpretMatch: KeySym "+" Expr  */
#line 464 "src/xkbcomp/parser.y"
                        { (yyval.interp) = InterpCreate((yyvsp[-2].keysym), (yyvsp[0].expr)); }
#line 2544 "parser.c"
    break;

  case 58: /* InterpretMatch: KeySym  */
#line 466 "src/xkbcomp/parser.y"
                        { (yyval.interp) = InterpCreate((yyvsp[0].keysym), NULL); }
#line 2550 "parser.c"
    break;

  case 59: /* VarDeclList: VarDeclList VarDecl  */
#line 470 "src/xkbcomp/parser.y"
                        {
                            if ((yyvsp[0].var)) {
                                if ((yyvsp[-1].varList).head) {
                                    (yyval.varList).head = (yyvsp[-1].varList).head;
                                    (yyval.varList).last->common.next = &(yyvsp[0].var)->common;
                                    (yyval.varList).last = (yyvsp[0].var);
                                } else {
                                    (yyval.varList).head = (yyval.varList).last = (yyvsp[0].var);
                                }
                            }
                        }
#line 2566 "parser.c"
    break;

  case 60: /* VarDeclList: %empty  */
#line 481 "src/xkbcomp/parser.y"
                        { (yyval.varList).head = (yyval.varList).last = NULL; }
#line 2572 "parser.c"
    break;

  case 61: /* KeyTypeDecl: "type" String "{" VarDeclList "}" ";"  */
#line 487 "src/xkbcomp/parser.y"
                        { (yyval.keyType) = KeyTypeCreate((yyvsp[-4].atom), (yyvsp[-2].varList).head); }
#line 2578 "parser.c"
    break;

  case 62: /* SymbolsDecl: "key" "key name" "{" OptSymbolsBody "}" ";"  */
#line 493 "src/xkbcomp/parser.y"
                        { (yyval.syms) = SymbolsCreate((yyvsp[-4].atom), (yyvsp[-2].varList).head); }
#line 2584 "parser.c"
    break;

  case 63: /* OptSymbolsBody: SymbolsBody  */
#line 496 "src/xkbcomp/parser.y"
                                    { (yyval.varList) = (yyvsp[0].varList); }
#line 2590 "parser.c"
    break;

  case 64: /* OptSymbolsBody: %empty  */
#line 497 "src/xkbcomp/parser.y"
                                    { (yyval.varList).head = (yyval.varList).last = NULL; }
#line 2596 "parser.c"
    break;

  case 65: /* SymbolsBody: SymbolsBody "," SymbolsVarDecl  */
#line 501 "src/xkbcomp/parser.y"
                        { (yyval.varList).head = (yyvsp[-2].varList).head; (yyval.varList).last->common.next = &(yyvsp[0].var)->common; (yyval.varList).last = (yyvsp[0].var); }
#line 2602 "parser.c"
    break;

  case 66: /* SymbolsBody: SymbolsVarDecl  */
#line 503 "src/xkbcomp/parser.y"
                        { (yyval.varList).head = (yyval.varList).last = (yyvsp[0].var); }
#line 2608 "parser.c"
    break;

  case 67: /* SymbolsVarDecl: Lhs "=" Expr  */
#line 506 "src/xkbcomp/parser.y"
                                                { (yyval.var) = VarCreate((yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 2614 "parser.c"
    break;

  case 68: /* SymbolsVarDecl: Lhs "=" MultiKeySymOrActionList  */
#line 507 "src/xkbcomp/parser.y"
                                                           { (yyval.var) = VarCreate((yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 2620 "parser.c"
    break;

  case 69: /* SymbolsVarDecl: Ident  */
#line 508 "src/xkbcomp/parser.y"
                                                { (yyval.var) = BoolVarCreate((yyvsp[0].atom), true); }
#line 2626 "parser.c"
    break;

  case 70: /* SymbolsVarDecl: "!" Ident  */
#line 509 "src/xkbcomp/parser.y"
                                                { (yyval.var) = BoolVarCreate((yyvsp[0].atom), false); }
#line 2632 "parser.c"
    break;

  case 71: /* SymbolsVarDecl: MultiKeySymOrActionList  */
#line 510 "src/xkbcomp/parser.y"
                                                { (yyval.var) = VarCreate(NULL, (yyvsp[0].expr)); }
#line 2638 "parser.c"
    break;

  case 72: /* MultiKeySymOrActionList: "[" MultiKeySymList "]"  */
#line 526 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[-1].exprList).head; }
#line 2644 "parser.c"
    break;

  case 73: /* MultiKeySymOrActionList: "[" NoSymbolOrActionList "," MultiKeySymList "]"  */
#line 528 "src/xkbcomp/parser.y"
                        {
                            /* Prepend n times NoSymbol */
                            struct {ExprDef *head; ExprDef *last;} list = {
                                .head = (yyvsp[-1].exprList).head, .last = (yyvsp[-1].exprList).last
                            };
                            for (uint32_t k = 0; k < (yyvsp[-3].noSymbolOrActionList); k++) {
                                ExprDef* const syms =
                                    ExprCreateKeySymList(XKB_KEY_NoSymbol);
                                if (!syms) {
                                    /* TODO: Use Bison’s more appropriate YYNOMEM */
                                    YYABORT;
                                }
                                syms->common.next = &list.head->common;
                                list.head = syms;
                            }
                            (yyval.expr) = list.head;
                        }
#line 2666 "parser.c"
    break;

  case 74: /* MultiKeySymOrActionList: "[" MultiActionList "]"  */
#line 546 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[-1].exprList).head; }
#line 2672 "parser.c"
    break;

  case 75: /* MultiKeySymOrActionList: "[" NoSymbolOrActionList "," MultiActionList "]"  */
#line 548 "src/xkbcomp/parser.y"
                        {
                            /* Prepend n times NoAction() */
                            struct {ExprDef *head; ExprDef *last;} list = {
                                .head = (yyvsp[-1].exprList).head, .last = (yyvsp[-1].exprList).last
                            };
                            for (uint32_t k = 0; k < (yyvsp[-3].noSymbolOrActionList); k++) {
                                ExprDef* const acts = ExprCreateActionList(NULL);
                                if (!acts) {
                                    /* TODO: Use Bison’s more appropriate YYNOMEM */
                                    YYABORT;
                                }
                                acts->common.next = &list.head->common;
                                list.head = acts;
                            }
                            (yyval.expr) = list.head;
                        }
#line 2693 "parser.c"
    break;

  case 76: /* MultiKeySymOrActionList: "[" NoSymbolOrActionList "]"  */
#line 570 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprEmptyList(); }
#line 2699 "parser.c"
    break;

  case 77: /* NoSymbolOrActionList: NoSymbolOrActionList "," "{" "}"  */
#line 576 "src/xkbcomp/parser.y"
                        { (yyval.noSymbolOrActionList) = (yyvsp[-3].noSymbolOrActionList) + 1; }
#line 2705 "parser.c"
    break;

  case 78: /* NoSymbolOrActionList: "{" "}"  */
#line 578 "src/xkbcomp/parser.y"
                        { (yyval.noSymbolOrActionList) = 1; }
#line 2711 "parser.c"
    break;

  case 79: /* NoSymbolOrActionList: %empty  */
#line 579 "src/xkbcomp/parser.y"
                        { (yyval.noSymbolOrActionList) = 0; }
#line 2717 "parser.c"
    break;

  case 80: /* GroupCompatDecl: "group" Integer "=" Expr ";"  */
#line 583 "src/xkbcomp/parser.y"
                        { (yyval.groupCompat) = GroupCompatCreate((yyvsp[-3].num), (yyvsp[-1].expr)); }
#line 2723 "parser.c"
    break;

  case 81: /* ModMapDecl: "modifier_map" Ident "{" KeyOrKeySymList "}" ";"  */
#line 587 "src/xkbcomp/parser.y"
                        { (yyval.modMask) = ModMapCreate((yyvsp[-4].atom), (yyvsp[-2].exprList).head); }
#line 2729 "parser.c"
    break;

  case 82: /* KeyOrKeySymList: KeyOrKeySymList "," KeyOrKeySym  */
#line 591 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyvsp[-2].exprList).head; (yyval.exprList).last->common.next = &(yyvsp[0].expr)->common; (yyval.exprList).last = (yyvsp[0].expr); }
#line 2735 "parser.c"
    break;

  case 83: /* KeyOrKeySymList: KeyOrKeySym  */
#line 593 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr); }
#line 2741 "parser.c"
    break;

  case 84: /* KeyOrKeySym: "key name"  */
#line 597 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateKeyName((yyvsp[0].atom)); }
#line 2747 "parser.c"
    break;

  case 85: /* KeyOrKeySym: KeySym  */
#line 599 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateKeySym((yyvsp[0].keysym)); }
#line 2753 "parser.c"
    break;

  case 86: /* LedMapDecl: "indicator" String "{" VarDeclList "}" ";"  */
#line 603 "src/xkbcomp/parser.y"
                        { (yyval.ledMap) = LedMapCreate((yyvsp[-4].atom), (yyvsp[-2].varList).head); }
#line 2759 "parser.c"
    break;

  case 87: /* LedNameDecl: "indicator" Integer "=" Expr ";"  */
#line 607 "src/xkbcomp/parser.y"
                        { (yyval.ledName) = LedNameCreate((yyvsp[-3].num), (yyvsp[-1].expr), false); }
#line 2765 "parser.c"
    break;

  case 88: /* LedNameDecl: "virtual" "indicator" Integer "=" Expr ";"  */
#line 609 "src/xkbcomp/parser.y"
                        { (yyval.ledName) = LedNameCreate((yyvsp[-3].num), (yyvsp[-1].expr), true); }
#line 2771 "parser.c"
    break;

  case 89: /* ShapeDecl: "shape" String "{" OutlineList "}" ";"  */
#line 613 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2777 "parser.c"
    break;

  case 90: /* ShapeDecl: "shape" String "{" CoordList "}" ";"  */
#line 615 "src/xkbcomp/parser.y"
                        { (void) (yyvsp[-2].expr); (yyval.geom) = NULL; }
#line 2783 "parser.c"
    break;

  case 91: /* SectionDecl: "section" String "{" SectionBody "}" ";"  */
#line 619 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2789 "parser.c"
    break;

  case 92: /* SectionBody: SectionBody SectionBodyItem  */
#line 622 "src/xkbcomp/parser.y"
                                                        { (yyval.geom) = NULL;}
#line 2795 "parser.c"
    break;

  case 93: /* SectionBody: SectionBodyItem  */
#line 623 "src/xkbcomp/parser.y"
                                                        { (yyval.geom) = NULL; }
#line 2801 "parser.c"
    break;

  case 94: /* SectionBodyItem: "row" "{" RowBody "}" ";"  */
#line 627 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2807 "parser.c"
    break;

  case 95: /* SectionBodyItem: VarDecl  */
#line 629 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[0].var)); (yyval.geom) = NULL; }
#line 2813 "parser.c"
    break;

  case 96: /* SectionBodyItem: DoodadDecl  */
#line 631 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2819 "parser.c"
    break;

  case 97: /* SectionBodyItem: LedMapDecl  */
#line 633 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[0].ledMap)); (yyval.geom) = NULL; }
#line 2825 "parser.c"
    break;

  case 98: /* SectionBodyItem: OverlayDecl  */
#line 635 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2831 "parser.c"
    break;

  case 99: /* RowBody: RowBody RowBodyItem  */
#line 638 "src/xkbcomp/parser.y"
                                                { (yyval.geom) = NULL;}
#line 2837 "parser.c"
    break;

  case 100: /* RowBody: RowBodyItem  */
#line 639 "src/xkbcomp/parser.y"
                                                { (yyval.geom) = NULL; }
#line 2843 "parser.c"
    break;

  case 101: /* RowBodyItem: "keys" "{" Keys "}" ";"  */
#line 642 "src/xkbcomp/parser.y"
                                                     { (yyval.geom) = NULL; }
#line 2849 "parser.c"
    break;

  case 102: /* RowBodyItem: VarDecl  */
#line 644 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[0].var)); (yyval.geom) = NULL; }
#line 2855 "parser.c"
    break;

  case 103: /* Keys: Keys "," Key  */
#line 647 "src/xkbcomp/parser.y"
                                                { (yyval.geom) = NULL; }
#line 2861 "parser.c"
    break;

  case 104: /* Keys: Key  */
#line 648 "src/xkbcomp/parser.y"
                                                { (yyval.geom) = NULL; }
#line 2867 "parser.c"
    break;

  case 105: /* Key: "key name"  */
#line 652 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2873 "parser.c"
    break;

  case 106: /* Key: "{" ExprList "}"  */
#line 654 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[-1].exprList).head); (yyval.geom) = NULL; }
#line 2879 "parser.c"
    break;

  case 107: /* OverlayDecl: "overlay" String "{" OverlayKeyList "}" ";"  */
#line 658 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2885 "parser.c"
    break;

  case 108: /* OverlayKeyList: OverlayKeyList "," OverlayKey  */
#line 661 "src/xkbcomp/parser.y"
                                                        { (yyval.geom) = NULL; }
#line 2891 "parser.c"
    break;

  case 109: /* OverlayKeyList: OverlayKey  */
#line 662 "src/xkbcomp/parser.y"
                                                        { (yyval.geom) = NULL; }
#line 2897 "parser.c"
    break;

  case 110: /* OverlayKey: "key name" "=" "key name"  */
#line 665 "src/xkbcomp/parser.y"
                                                        { (yyval.geom) = NULL; }
#line 2903 "parser.c"
    break;

  case 111: /* OutlineList: OutlineList "," OutlineInList  */
#line 669 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL;}
#line 2909 "parser.c"
    break;

  case 112: /* OutlineList: OutlineInList  */
#line 671 "src/xkbcomp/parser.y"
                        { (yyval.geom) = NULL; }
#line 2915 "parser.c"
    break;

  case 113: /* OutlineInList: "{" CoordList "}"  */
#line 675 "src/xkbcomp/parser.y"
                        { (void) (yyvsp[-1].expr); (yyval.geom) = NULL; }
#line 2921 "parser.c"
    break;

  case 114: /* OutlineInList: Ident "=" "{" CoordList "}"  */
#line 677 "src/xkbcomp/parser.y"
                        { (void) (yyvsp[-1].expr); (yyval.geom) = NULL; }
#line 2927 "parser.c"
    break;

  case 115: /* OutlineInList: Ident "=" Expr  */
#line 679 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[0].expr)); (yyval.geom) = NULL; }
#line 2933 "parser.c"
    break;

  case 116: /* CoordList: CoordList "," Coord  */
#line 683 "src/xkbcomp/parser.y"
                        { (void) (yyvsp[-2].expr); (void) (yyvsp[0].expr); (yyval.expr) = NULL; }
#line 2939 "parser.c"
    break;

  case 117: /* CoordList: Coord  */
#line 685 "src/xkbcomp/parser.y"
                        { (void) (yyvsp[0].expr); (yyval.expr) = NULL; }
#line 2945 "parser.c"
    break;

  case 118: /* Coord: "[" SignedNumber "," SignedNumber "]"  */
#line 689 "src/xkbcomp/parser.y"
                        { (yyval.expr) = NULL; }
#line 2951 "parser.c"
    break;

  case 119: /* DoodadDecl: DoodadType String "{" VarDeclList "}" ";"  */
#line 693 "src/xkbcomp/parser.y"
                        { FreeStmt((ParseCommon *) (yyvsp[-2].varList).head); (yyval.geom) = NULL; }
#line 2957 "parser.c"
    break;

  case 120: /* DoodadType: "text"  */
#line 696 "src/xkbcomp/parser.y"
                                { (yyval.num) = 0; }
#line 2963 "parser.c"
    break;

  case 121: /* DoodadType: "outline"  */
#line 697 "src/xkbcomp/parser.y"
                                { (yyval.num) = 0; }
#line 2969 "parser.c"
    break;

  case 122: /* DoodadType: "solid"  */
#line 698 "src/xkbcomp/parser.y"
                                { (yyval.num) = 0; }
#line 2975 "parser.c"
    break;

  case 123: /* DoodadType: "logo"  */
#line 699 "src/xkbcomp/parser.y"
                                { (yyval.num) = 0; }
#line 2981 "parser.c"
    break;

  case 124: /* FieldSpec: Ident  */
#line 702 "src/xkbcomp/parser.y"
                                { (yyval.atom) = (yyvsp[0].atom); }
#line 2987 "parser.c"
    break;

  case 125: /* FieldSpec: Element  */
#line 703 "src/xkbcomp/parser.y"
                                { (yyval.atom) = (yyvsp[0].atom); }
#line 2993 "parser.c"
    break;

  case 126: /* Element: "action"  */
#line 707 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "action"); }
#line 2999 "parser.c"
    break;

  case 127: /* Element: "interpret"  */
#line 709 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "interpret"); }
#line 3005 "parser.c"
    break;

  case 128: /* Element: "type"  */
#line 711 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "type"); }
#line 3011 "parser.c"
    break;

  case 129: /* Element: "key"  */
#line 713 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "key"); }
#line 3017 "parser.c"
    break;

  case 130: /* Element: "group"  */
#line 715 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "group"); }
#line 3023 "parser.c"
    break;

  case 131: /* Element: "modifier_map"  */
#line 717 "src/xkbcomp/parser.y"
                        {(yyval.atom) = xkb_atom_intern_literal(param->ctx, "modifier_map");}
#line 3029 "parser.c"
    break;

  case 132: /* Element: "indicator"  */
#line 719 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "indicator"); }
#line 3035 "parser.c"
    break;

  case 133: /* Element: "shape"  */
#line 721 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "shape"); }
#line 3041 "parser.c"
    break;

  case 134: /* Element: "row"  */
#line 723 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "row"); }
#line 3047 "parser.c"
    break;

  case 135: /* Element: "section"  */
#line 725 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "section"); }
#line 3053 "parser.c"
    break;

  case 136: /* Element: "text"  */
#line 727 "src/xkbcomp/parser.y"
                        { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "text"); }
#line 3059 "parser.c"
    break;

  case 137: /* OptMergeMode: MergeMode  */
#line 730 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = (yyvsp[0].merge); }
#line 3065 "parser.c"
    break;

  case 138: /* OptMergeMode: %empty  */
#line 731 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = MERGE_DEFAULT; }
#line 3071 "parser.c"
    break;

  case 139: /* MergeMode: "include"  */
#line 734 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = MERGE_DEFAULT; }
#line 3077 "parser.c"
    break;

  case 140: /* MergeMode: "augment"  */
#line 735 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = MERGE_AUGMENT; }
#line 3083 "parser.c"
    break;

  case 141: /* MergeMode: "override"  */
#line 736 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = MERGE_OVERRIDE; }
#line 3089 "parser.c"
    break;

  case 142: /* MergeMode: "replace"  */
#line 737 "src/xkbcomp/parser.y"
                                        { (yyval.merge) = MERGE_REPLACE; }
#line 3095 "parser.c"
    break;

  case 143: /* MergeMode: "alternate"  */
#line 739 "src/xkbcomp/parser.y"
                {
                    /*
                     * This used to be MERGE_ALT_FORM. This functionality was
                     * unused and has been removed.
                     */
                    parser_warn(param, XKB_LOG_MESSAGE_NO_ID,
                                "ignored unsupported legacy merge mode \"alternate\"");
                    (yyval.merge) = MERGE_DEFAULT;
                }
#line 3109 "parser.c"
    break;

  case 144: /* ExprList: ExprList "," Expr  */
#line 751 "src/xkbcomp/parser.y"
                        {
                            if ((yyvsp[0].expr)) {
                                if ((yyvsp[-2].exprList).head) {
                                    (yyval.exprList).head = (yyvsp[-2].exprList).head;
                                    (yyval.exprList).last->common.next = &(yyvsp[0].expr)->common;
                                    (yyval.exprList).last = (yyvsp[0].expr);
                                } else {
                                    (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr);
                                }
                            }
                        }
#line 3125 "parser.c"
    break;

  case 145: /* ExprList: Expr  */
#line 763 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr); }
#line 3131 "parser.c"
    break;

  case 146: /* ExprList: %empty  */
#line 764 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = NULL; }
#line 3137 "parser.c"
    break;

  case 147: /* Expr: Expr "/" Expr  */
#line 768 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateBinary(STMT_EXPR_DIVIDE, (yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 3143 "parser.c"
    break;

  case 148: /* Expr: Expr "+" Expr  */
#line 770 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateBinary(STMT_EXPR_ADD, (yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 3149 "parser.c"
    break;

  case 149: /* Expr: Expr "-" Expr  */
#line 772 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateBinary(STMT_EXPR_SUBTRACT, (yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 3155 "parser.c"
    break;

  case 150: /* Expr: Expr "*" Expr  */
#line 774 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateBinary(STMT_EXPR_MULTIPLY, (yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 3161 "parser.c"
    break;

  case 151: /* Expr: Lhs "=" Expr  */
#line 776 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateBinary(STMT_EXPR_ASSIGN, (yyvsp[-2].expr), (yyvsp[0].expr)); }
#line 3167 "parser.c"
    break;

  case 152: /* Expr: Term  */
#line 778 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3173 "parser.c"
    break;

  case 153: /* Term: "-" Term  */
#line 782 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateUnary(STMT_EXPR_NEGATE, (yyvsp[0].expr)); }
#line 3179 "parser.c"
    break;

  case 154: /* Term: "+" Term  */
#line 784 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateUnary(STMT_EXPR_UNARY_PLUS, (yyvsp[0].expr)); }
#line 3185 "parser.c"
    break;

  case 155: /* Term: "!" Term  */
#line 786 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateUnary(STMT_EXPR_NOT, (yyvsp[0].expr)); }
#line 3191 "parser.c"
    break;

  case 156: /* Term: "~" Term  */
#line 788 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateUnary(STMT_EXPR_INVERT, (yyvsp[0].expr)); }
#line 3197 "parser.c"
    break;

  case 157: /* Term: Lhs  */
#line 790 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3203 "parser.c"
    break;

  case 158: /* Term: FieldSpec "(" ExprList ")"  */
#line 792 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateAction((yyvsp[-3].atom), (yyvsp[-1].exprList).head); }
#line 3209 "parser.c"
    break;

  case 159: /* Term: Actions  */
#line 794 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3215 "parser.c"
    break;

  case 160: /* Term: Terminal  */
#line 796 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3221 "parser.c"
    break;

  case 161: /* Term: "(" Expr ")"  */
#line 798 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[-1].expr); }
#line 3227 "parser.c"
    break;

  case 162: /* MultiActionList: MultiActionList "," Action  */
#line 802 "src/xkbcomp/parser.y"
                        {
                            ExprDef *expr = ExprCreateActionList((yyvsp[0].expr));
                            (yyval.exprList) = (yyvsp[-2].exprList);
                            (yyval.exprList).last->common.next = &expr->common; (yyval.exprList).last = expr;
                        }
#line 3237 "parser.c"
    break;

  case 163: /* MultiActionList: MultiActionList "," Actions  */
#line 808 "src/xkbcomp/parser.y"
                        { (yyval.exprList) = (yyvsp[-2].exprList); (yyval.exprList).last->common.next = &(yyvsp[0].expr)->common; (yyval.exprList).last = (yyvsp[0].expr); }
#line 3243 "parser.c"
    break;

  case 164: /* MultiActionList: Action  */
#line 810 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = ExprCreateActionList((yyvsp[0].expr)); }
#line 3249 "parser.c"
    break;

  case 165: /* MultiActionList: NonEmptyActions  */
#line 812 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr); }
#line 3255 "parser.c"
    break;

  case 166: /* ActionList: ActionList "," Action  */
#line 816 "src/xkbcomp/parser.y"
                        { (yyval.exprList) = (yyvsp[-2].exprList); (yyval.exprList).last->common.next = &(yyvsp[0].expr)->common; (yyval.exprList).last = (yyvsp[0].expr); }
#line 3261 "parser.c"
    break;

  case 167: /* ActionList: Action  */
#line 818 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr); }
#line 3267 "parser.c"
    break;

  case 168: /* NonEmptyActions: "{" ActionList "}"  */
#line 822 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateActionList((yyvsp[-1].exprList).head); }
#line 3273 "parser.c"
    break;

  case 169: /* Actions: NonEmptyActions  */
#line 826 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3279 "parser.c"
    break;

  case 170: /* Actions: "{" "}"  */
#line 828 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateActionList(NULL); }
#line 3285 "parser.c"
    break;

  case 171: /* Action: FieldSpec "(" ExprList ")"  */
#line 832 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateAction((yyvsp[-3].atom), (yyvsp[-1].exprList).head); }
#line 3291 "parser.c"
    break;

  case 172: /* Lhs: FieldSpec  */
#line 836 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateIdent((yyvsp[0].atom)); }
#line 3297 "parser.c"
    break;

  case 173: /* Lhs: FieldSpec "." FieldSpec  */
#line 838 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateFieldRef((yyvsp[-2].atom), (yyvsp[0].atom)); }
#line 3303 "parser.c"
    break;

  case 174: /* Lhs: FieldSpec "[" Expr "]"  */
#line 840 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateArrayRef(XKB_ATOM_NONE, (yyvsp[-3].atom), (yyvsp[-1].expr)); }
#line 3309 "parser.c"
    break;

  case 175: /* Lhs: FieldSpec "." FieldSpec "[" Expr "]"  */
#line 842 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateArrayRef((yyvsp[-5].atom), (yyvsp[-3].atom), (yyvsp[-1].expr)); }
#line 3315 "parser.c"
    break;

  case 176: /* Terminal: String  */
#line 846 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateString((yyvsp[0].atom)); }
#line 3321 "parser.c"
    break;

  case 177: /* Terminal: Integer  */
#line 848 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateInteger((yyvsp[0].num)); }
#line 3327 "parser.c"
    break;

  case 178: /* Terminal: Float  */
#line 850 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateFloat(/* Discard $1 */); }
#line 3333 "parser.c"
    break;

  case 179: /* Terminal: "key name"  */
#line 852 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateKeyName((yyvsp[0].atom)); }
#line 3339 "parser.c"
    break;

  case 180: /* MultiKeySymList: MultiKeySymList "," KeySymLit  */
#line 856 "src/xkbcomp/parser.y"
                        {
                            ExprDef *expr = ExprCreateKeySymList((yyvsp[0].keysym));
                            (yyval.exprList) = (yyvsp[-2].exprList);
                            (yyval.exprList).last->common.next = &expr->common; (yyval.exprList).last = expr;
                        }
#line 3349 "parser.c"
    break;

  case 181: /* MultiKeySymList: MultiKeySymList "," KeySyms  */
#line 862 "src/xkbcomp/parser.y"
                        { (yyval.exprList) = (yyvsp[-2].exprList); (yyval.exprList).last->common.next = &(yyvsp[0].expr)->common; (yyval.exprList).last = (yyvsp[0].expr); }
#line 3355 "parser.c"
    break;

  case 182: /* MultiKeySymList: KeySymLit  */
#line 864 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = ExprCreateKeySymList((yyvsp[0].keysym)); }
#line 3361 "parser.c"
    break;

  case 183: /* MultiKeySymList: NonEmptyKeySyms  */
#line 866 "src/xkbcomp/parser.y"
                        { (yyval.exprList).head = (yyval.exprList).last = (yyvsp[0].expr); }
#line 3367 "parser.c"
    break;

  case 184: /* KeySymList: KeySymList "," KeySymLit  */
#line 870 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprAppendKeySymList((yyvsp[-2].expr), (yyvsp[0].keysym)); }
#line 3373 "parser.c"
    break;

  case 185: /* KeySymList: KeySymList "," "string literal"  */
#line 872 "src/xkbcomp/parser.y"
                        {
                            (yyval.expr) = ExprKeySymListAppendString(param->scanner, (yyvsp[-2].expr), (yyvsp[0].str));
                            free((yyvsp[0].str));
                            if (!(yyval.expr))
                                YYERROR;
                        }
#line 3384 "parser.c"
    break;

  case 186: /* KeySymList: KeySymLit  */
#line 879 "src/xkbcomp/parser.y"
                        {
                            (yyval.expr) = ExprCreateKeySymList((yyvsp[0].keysym));
                            if (!(yyval.expr))
                                YYERROR;
                        }
#line 3394 "parser.c"
    break;

  case 187: /* KeySymList: "string literal"  */
#line 885 "src/xkbcomp/parser.y"
                        {
                            (yyval.expr) = ExprCreateKeySymList(XKB_KEY_NoSymbol);
                            if (!(yyval.expr))
                                YYERROR;
                            (yyval.expr) = ExprKeySymListAppendString(param->scanner, (yyval.expr), (yyvsp[0].str));
                            free((yyvsp[0].str));
                            if (!(yyval.expr))
                                YYERROR;
                        }
#line 3408 "parser.c"
    break;

  case 188: /* NonEmptyKeySyms: "{" KeySymList "}"  */
#line 897 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[-1].expr); }
#line 3414 "parser.c"
    break;

  case 189: /* NonEmptyKeySyms: "string literal"  */
#line 899 "src/xkbcomp/parser.y"
                        {
                            (yyval.expr) = ExprCreateKeySymList(XKB_KEY_NoSymbol);
                            if (!(yyval.expr))
                                YYERROR;
                            (yyval.expr) = ExprKeySymListAppendString(param->scanner, (yyval.expr), (yyvsp[0].str));
                            free((yyvsp[0].str));
                            if (!(yyval.expr))
                                YYERROR;
                        }
#line 3428 "parser.c"
    break;

  case 190: /* KeySyms: NonEmptyKeySyms  */
#line 911 "src/xkbcomp/parser.y"
                        { (yyval.expr) = (yyvsp[0].expr); }
#line 3434 "parser.c"
    break;

  case 191: /* KeySyms: "{" "}"  */
#line 913 "src/xkbcomp/parser.y"
                        { (yyval.expr) = ExprCreateKeySymList(XKB_KEY_NoSymbol); }
#line 3440 "parser.c"
    break;

  case 192: /* KeySym: KeySymLit  */
#line 917 "src/xkbcomp/parser.y"
                        { (yyval.keysym) = (yyvsp[0].keysym); }
#line 3446 "parser.c"
    break;

  case 193: /* KeySym: "string literal"  */
#line 919 "src/xkbcomp/parser.y"
                        {
                            (yyval.keysym) = KeysymParseString(param->scanner, (yyvsp[0].str));
                            free((yyvsp[0].str));
                            if ((yyval.keysym) == XKB_KEY_NoSymbol)
                                YYERROR;
                        }
#line 3457 "parser.c"
    break;

  case 194: /* KeySymLit: "identifier"  */
#line 928 "src/xkbcomp/parser.y"
                        {
                            if (!resolve_keysym(param, (yyvsp[0].sval), &(yyval.keysym))) {
                                parser_warn(
                                    param,
                                    XKB_WARNING_UNRECOGNIZED_KEYSYM,
                                    "unrecognized keysym \"%.*s\"",
                                    (unsigned int) (yyvsp[0].sval).len, (yyvsp[0].sval).start
                                );
                                (yyval.keysym) = XKB_KEY_NoSymbol;
                            }
                        }
#line 3473 "parser.c"
    break;

  case 195: /* KeySymLit: "section"  */
#line 940 "src/xkbcomp/parser.y"
                                { (yyval.keysym) = XKB_KEY_section; }
#line 3479 "parser.c"
    break;

  case 196: /* KeySymLit: "decimal digit"  */
#line 942 "src/xkbcomp/parser.y"
                        {
                            /*
                             * Special case for digits 0..9:
                             * map to XKB_KEY_0 .. XKB_KEY_9, consistent with
                             * other keysym names: <name> → XKB_KEY_<name>.
                             */
                            (yyval.keysym) = XKB_KEY_0 + (xkb_keysym_t) (yyvsp[0].num);
                        }
#line 3492 "parser.c"
    break;

  case 197: /* KeySymLit: "integer literal"  */
#line 951 "src/xkbcomp/parser.y"
                        {
                            if ((yyvsp[0].num) < XKB_KEYSYM_MIN) {
                                /* Negative value */
                                static_assert(XKB_KEYSYM_MIN == 0,
                                              "Keysyms are positive");
                                parser_warn(
                                    param,
                                    XKB_ERROR_INVALID_NUMERIC_KEYSYM,
                                    "unrecognized keysym \"-%#06"PRIx64"\""
                                    " (%"PRId64")",
                                    -(yyvsp[0].num), (yyvsp[0].num)
                                );
                                (yyval.keysym) = XKB_KEY_NoSymbol;
                            }
                            /*
                             * Integers 0..9 are handled with DECIMAL_DIGIT if
                             * they were formatted as single characters '0'..'9'.
                             * Otherwise they are handled here as raw keysyms
                             * values. E.g. `01` and `0x1` are interpreted as
                             * the keysym 0x0001, while `1` is interpreted as
                             * XKB_KEY_1.
                             */
                            else {
                                /* Any other numeric value */
                                if ((yyvsp[0].num) <= XKB_KEYSYM_MAX) {
                                    /*
                                     * Valid keysym
                                     * No normalization is performed and value
                                     * is used as is.
                                     */
                                    (yyval.keysym) = (xkb_keysym_t) (yyvsp[0].num);
                                    check_deprecated_keysyms(
                                        parser_warn, param, param->ctx,
                                        (yyval.keysym), NULL, (yyval.keysym), "%#06"PRIx32, "");
                                } else {
                                    /* Invalid keysym */
                                    parser_warn(
                                        param, XKB_ERROR_INVALID_NUMERIC_KEYSYM,
                                        "unrecognized keysym \"%#06"PRIx64"\" "
                                        "(%"PRId64")", (yyvsp[0].num), (yyvsp[0].num)
                                    );
                                    (yyval.keysym) = XKB_KEY_NoSymbol;
                                }
                                parser_vrb(
                                    /*
                                     * Require an extra high verbosity, because
                                     * keysyms are formatted as number unless
                                     * enabling pretty-pretting for the
                                     * serialization.
                                     */
                                    param, XKB_LOG_VERBOSITY_COMPREHENSIVE,
                                    XKB_WARNING_NUMERIC_KEYSYM,
                                    "numeric keysym \"%#06"PRIx64"\" (%"PRId64")",
                                    (yyvsp[0].num), (yyvsp[0].num)
                                );
                            }
                        }
#line 3554 "parser.c"
    break;

  case 198: /* SignedNumber: "-" Number  */
#line 1010 "src/xkbcomp/parser.y"
                                        { (yyval.num) = -(yyvsp[0].num); }
#line 3560 "parser.c"
    break;

  case 199: /* SignedNumber: Number  */
#line 1011 "src/xkbcomp/parser.y"
                                        { (yyval.num) = (yyvsp[0].num); }
#line 3566 "parser.c"
    break;

  case 200: /* Number: "float literal"  */
#line 1014 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3572 "parser.c"
    break;

  case 201: /* Number: "decimal digit"  */
#line 1015 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3578 "parser.c"
    break;

  case 202: /* Number: "integer literal"  */
#line 1016 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3584 "parser.c"
    break;

  case 203: /* Float: "float literal"  */
#line 1019 "src/xkbcomp/parser.y"
                                { (yyval.num) = 0; }
#line 3590 "parser.c"
    break;

  case 204: /* Integer: "integer literal"  */
#line 1022 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3596 "parser.c"
    break;

  case 205: /* Integer: "decimal digit"  */
#line 1023 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3602 "parser.c"
    break;

  case 206: /* KeyCode: "integer literal"  */
#line 1026 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3608 "parser.c"
    break;

  case 207: /* KeyCode: "decimal digit"  */
#line 1027 "src/xkbcomp/parser.y"
                                      { (yyval.num) = (yyvsp[0].num); }
#line 3614 "parser.c"
    break;

  case 208: /* Ident: "identifier"  */
#line 1030 "src/xkbcomp/parser.y"
                                { (yyval.atom) = xkb_atom_intern(param->ctx, (yyvsp[0].sval).start, (yyvsp[0].sval).len); }
#line 3620 "parser.c"
    break;

  case 209: /* Ident: "default"  */
#line 1031 "src/xkbcomp/parser.y"
                                { (yyval.atom) = xkb_atom_intern_literal(param->ctx, "default"); }
#line 3626 "parser.c"
    break;

  case 210: /* String: "string literal"  */
#line 1034 "src/xkbcomp/parser.y"
                                { (yyval.atom) = xkb_atom_intern(param->ctx, (yyvsp[0].str), strlen((yyvsp[0].str))); free((yyvsp[0].str)); }
#line 3632 "parser.c"
    break;

  case 211: /* OptMapName: MapName  */
#line 1037 "src/xkbcomp/parser.y"
                                { (yyval.str) = (yyvsp[0].str); }
#line 3638 "parser.c"
    break;

  case 212: /* OptMapName: %empty  */
#line 1038 "src/xkbcomp/parser.y"
                                { (yyval.str) = NULL; }
#line 3644 "parser.c"
    break;

  case 213: /* MapName: "string literal"  */
#line 1041 "src/xkbcomp/parser.y"
                                { (yyval.str) = (yyvsp[0].str); }
#line 3650 "parser.c"
    break;


#line 3654 "parser.c"

      default: break;
    }
  /* User semantic actions sometimes alter yychar, and that requires
     that yytoken be updated with the new translation.  We take the
     approach of translating immediately before every use of yytoken.
     One alternative is translating here after every semantic action,
     but that translation would be missed if the semantic action invokes
     YYABORT, YYACCEPT, or YYERROR immediately after altering yychar or
     if it invokes YYBACKUP.  In the case of YYABORT or YYACCEPT, an
     incorrect destructor might then be invoked immediately.  In the
     case of YYERROR or YYBACKUP, subsequent parser actions might lead
     to an incorrect destructor call or verbose syntax error message
     before the lookahead is translated.  */
  YY_SYMBOL_PRINT ("-> $$ =", YY_CAST (yysymbol_kind_t, yyr1[yyn]), &yyval, &yyloc);

  YYPOPSTACK (yylen);
  yylen = 0;

  *++yyvsp = yyval;

  /* Now 'shift' the result of the reduction.  Determine what state
     that goes to, based on the state we popped back to and the rule
     number reduced by.  */
  {
    const int yylhs = yyr1[yyn] - YYNTOKENS;
    const int yyi = yypgoto[yylhs] + *yyssp;
    yystate = (0 <= yyi && yyi <= YYLAST && yycheck[yyi] == *yyssp
               ? yytable[yyi]
               : yydefgoto[yylhs]);
  }

  goto yynewstate;


/*--------------------------------------.
| yyerrlab -- here on detecting error.  |
`--------------------------------------*/
yyerrlab:
  /* Make sure we have latest lookahead translation.  See comments at
     user semantic actions for why this is necessary.  */
  yytoken = yychar == YYEMPTY ? YYSYMBOL_YYEMPTY : YYTRANSLATE (yychar);
  /* If not already recovering from an error, report this error.  */
  if (!yyerrstatus)
    {
      ++yynerrs;
      {
        yypcontext_t yyctx
          = {yyssp, yytoken};
        char const *yymsgp = YY_("syntax error");
        int yysyntax_error_status;
        yysyntax_error_status = yysyntax_error (&yymsg_alloc, &yymsg, &yyctx);
        if (yysyntax_error_status == 0)
          yymsgp = yymsg;
        else if (yysyntax_error_status == -1)
          {
            if (yymsg != yymsgbuf)
              YYSTACK_FREE (yymsg);
            yymsg = YY_CAST (char *,
                             YYSTACK_ALLOC (YY_CAST (YYSIZE_T, yymsg_alloc)));
            if (yymsg)
              {
                yysyntax_error_status
                  = yysyntax_error (&yymsg_alloc, &yymsg, &yyctx);
                yymsgp = yymsg;
              }
            else
              {
                yymsg = yymsgbuf;
                yymsg_alloc = sizeof yymsgbuf;
                yysyntax_error_status = YYENOMEM;
              }
          }
        yyerror (param, yymsgp);
        if (yysyntax_error_status == YYENOMEM)
          YYNOMEM;
      }
    }

  if (yyerrstatus == 3)
    {
      /* If just tried and failed to reuse lookahead token after an
         error, discard it.  */

      if (yychar <= END_OF_FILE)
        {
          /* Return failure if at end of input.  */
          if (yychar == END_OF_FILE)
            YYABORT;
        }
      else
        {
          yydestruct ("Error: discarding",
                      yytoken, &yylval, param);
          yychar = YYEMPTY;
        }
    }

  /* Else will try to reuse lookahead token after shifting the error
     token.  */
  goto yyerrlab1;


/*---------------------------------------------------.
| yyerrorlab -- error raised explicitly by YYERROR.  |
`---------------------------------------------------*/
yyerrorlab:
  /* Pacify compilers when the user code never invokes YYERROR and the
     label yyerrorlab therefore never appears in user code.  */
  if (0)
    YYERROR;
  ++yynerrs;

  /* Do not reclaim the symbols of the rule whose action triggered
     this YYERROR.  */
  YYPOPSTACK (yylen);
  yylen = 0;
  YY_STACK_PRINT (yyss, yyssp);
  yystate = *yyssp;
  goto yyerrlab1;


/*-------------------------------------------------------------.
| yyerrlab1 -- common code for both syntax error and YYERROR.  |
`-------------------------------------------------------------*/
yyerrlab1:
  yyerrstatus = 3;      /* Each real token shifted decrements this.  */

  /* Pop stack until we find a state that shifts the error token.  */
  for (;;)
    {
      yyn = yypact[yystate];
      if (!yypact_value_is_default (yyn))
        {
          yyn += YYSYMBOL_YYerror;
          if (0 <= yyn && yyn <= YYLAST && yycheck[yyn] == YYSYMBOL_YYerror)
            {
              yyn = yytable[yyn];
              if (0 < yyn)
                break;
            }
        }

      /* Pop the current state because it cannot handle the error token.  */
      if (yyssp == yyss)
        YYABORT;


      yydestruct ("Error: popping",
                  YY_ACCESSING_SYMBOL (yystate), yyvsp, param);
      YYPOPSTACK (1);
      yystate = *yyssp;
      YY_STACK_PRINT (yyss, yyssp);
    }

  YY_IGNORE_MAYBE_UNINITIALIZED_BEGIN
  *++yyvsp = yylval;
  YY_IGNORE_MAYBE_UNINITIALIZED_END


  /* Shift the error token.  */
  YY_SYMBOL_PRINT ("Shifting", YY_ACCESSING_SYMBOL (yyn), yyvsp, yylsp);

  yystate = yyn;
  goto yynewstate;


/*-------------------------------------.
| yyacceptlab -- YYACCEPT comes here.  |
`-------------------------------------*/
yyacceptlab:
  yyresult = 0;
  goto yyreturnlab;


/*-----------------------------------.
| yyabortlab -- YYABORT comes here.  |
`-----------------------------------*/
yyabortlab:
  yyresult = 1;
  goto yyreturnlab;


/*-----------------------------------------------------------.
| yyexhaustedlab -- YYNOMEM (memory exhaustion) comes here.  |
`-----------------------------------------------------------*/
yyexhaustedlab:
  yyerror (param, YY_("memory exhausted"));
  yyresult = 2;
  goto yyreturnlab;


/*----------------------------------------------------------.
| yyreturnlab -- parsing is finished, clean up and return.  |
`----------------------------------------------------------*/
yyreturnlab:
  if (yychar != YYEMPTY)
    {
      /* Make sure we have latest lookahead translation.  See comments at
         user semantic actions for why this is necessary.  */
      yytoken = YYTRANSLATE (yychar);
      yydestruct ("Cleanup: discarding lookahead",
                  yytoken, &yylval, param);
    }
  /* Do not reclaim the symbols of the rule whose action triggered
     this YYABORT or YYACCEPT.  */
  YYPOPSTACK (yylen);
  YY_STACK_PRINT (yyss, yyssp);
  while (yyssp != yyss)
    {
      yydestruct ("Cleanup: popping",
                  YY_ACCESSING_SYMBOL (+*yyssp), yyvsp, param);
      YYPOPSTACK (1);
    }
#ifndef yyoverflow
  if (yyss != yyssa)
    YYSTACK_FREE (yyss);
#endif
  if (yymsg != yymsgbuf)
    YYSTACK_FREE (yymsg);
  return yyresult;
}

#line 1044 "src/xkbcomp/parser.y"


/* Parse a specific section */
XkbFile *
parse(struct xkb_context *ctx, struct scanner *scanner, const char *map)
{
    int ret;
    XkbFile *first = NULL;
    struct parser_param param = {
        .scanner = scanner,
        .ctx = ctx,
        .rtrn = NULL,
        .more_maps = false,
    };

    /*
     * If we got a specific map, we look for it exclusively and return
     * immediately upon finding it. Otherwise, we need to get the
     * default map. If we find a map marked as default, we return it
     * immediately. If there are no maps marked as default, we return
     * the first map in the file.
     */

    while ((ret = yyparse(&param)) == 0 && param.more_maps) {
        if (map) {
            if (streq_not_null(map, param.rtrn->name))
                return param.rtrn;
            else
                FreeXkbFile(param.rtrn);
        }
        else {
            if (param.rtrn->flags & MAP_IS_DEFAULT) {
                FreeXkbFile(first);
                return param.rtrn;
            }
            else if (!first) {
                first = param.rtrn;
            }
            else {
                FreeXkbFile(param.rtrn);
            }
        }
        param.rtrn = NULL;
    }

    if (ret != 0) {
        /* Some error happend; clear the Xkbfiles parsed so far */
        FreeXkbFile(first);
        FreeXkbFile(param.rtrn);
        return NULL;
    }

    if (first)
        log_vrb(ctx, XKB_LOG_VERBOSITY_DETAILED,
                XKB_WARNING_MISSING_DEFAULT_SECTION,
                "No map in include statement, but \"%s\" contains several; "
                "Using first defined map, \"%s\"\n",
                scanner->file_name, safe_map_name(first));

    return first;
}

/* Parse the next section */
bool
parse_next(struct xkb_context *ctx, struct scanner *scanner, XkbFile **xkb_file)
{
    int ret;
    struct parser_param param = {
        .scanner = scanner,
        .ctx = ctx,
        .rtrn = NULL,
        .more_maps = false,
    };

    if ((ret = yyparse(&param)) == 0 && param.more_maps) {
        *xkb_file = param.rtrn;
        return true;
    } else {
        FreeXkbFile(param.rtrn);
        *xkb_file = NULL;
        return (ret == 0);
    }
}
