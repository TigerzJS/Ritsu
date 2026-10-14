#include "filecheck.hpp"

#include <algorithm>
#include <cstddef>
#include <map>
#include <optional>
#include <regex>
#include <stdexcept>
#include <utility>
#include <vector>

namespace a2s_test {
namespace {

enum class Kind { Plain, Next, Same, Empty, Not, Dag };

const char* suffix_of(Kind kind) noexcept {
    switch (kind) {
    case Kind::Plain:
        return "";
    case Kind::Next:
        return "-NEXT";
    case Kind::Same:
        return "-SAME";
    case Kind::Empty:
        return "-EMPTY";
    case Kind::Not:
        return "-NOT";
    case Kind::Dag:
        return "-DAG";
    }
    return "";
}

struct Piece {
    enum class Type { Literal, Regex, Define, Use };
    Type type;
    std::string text; // literal text or regex
    std::string name; // variable name (Define/Use)
};

struct Directive {
    Kind kind;
    std::size_t check_line; // 1-based line in the check text
    std::string pattern;    // as written (for messages)
    std::vector<Piece> pieces;
};

// Thrown while parsing/compiling; turned into a FileCheckResult at the top level.
class CheckError : public std::runtime_error {
public:
    using std::runtime_error::runtime_error;
};

bool is_word_char(char c) noexcept {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' ||
           c == '-';
}

bool is_horizontal_space(char c) noexcept {
    return c == ' ' || c == '\t';
}

bool is_valid_name(std::string_view name) noexcept {
    if (name.empty() || (name[0] >= '0' && name[0] <= '9')) {
        return false;
    }
    return std::all_of(name.begin(), name.end(),
                       [](char c) { return is_word_char(c) && c != '-'; });
}

std::string_view trim(std::string_view text) noexcept {
    while (!text.empty() && is_horizontal_space(text.front())) {
        text.remove_prefix(1);
    }
    while (!text.empty() && (is_horizontal_space(text.back()) || text.back() == '\r')) {
        text.remove_suffix(1);
    }
    return text;
}

std::string where(std::size_t check_line, Kind kind, std::string_view prefix) {
    return "check line " + std::to_string(check_line) + ": " + std::string(prefix) +
           suffix_of(kind) + ": ";
}

// Offset of the "]]" that closes the "[[" at the start of `text`. Brackets inside the variable's
// regex are balanced ("[[N:[0-9]]]" ends at the last "]]"); backslash escapes are skipped.
std::size_t find_variable_end(std::string_view text) noexcept {
    std::size_t depth = 0;
    for (std::size_t i = 2; i < text.size(); ++i) {
        const char c = text[i];
        if (c == '\\') {
            ++i;
        } else if (c == '[') {
            ++depth;
        } else if (c == ']') {
            if (depth > 0) {
                --depth;
            } else if (i + 1 < text.size() && text[i + 1] == ']') {
                return i;
            }
        }
    }
    return std::string_view::npos;
}

std::vector<Piece> parse_pattern(std::string_view pattern, const std::string& context) {
    std::vector<Piece> pieces;
    std::string literal;
    auto flush_literal = [&] {
        if (!literal.empty()) {
            pieces.push_back(Piece{Piece::Type::Literal, std::move(literal), {}});
            literal.clear();
        }
    };
    std::size_t i = 0;
    while (i < pattern.size()) {
        const std::string_view rest = pattern.substr(i);
        if (rest.starts_with("{{")) {
            const std::size_t end = rest.find("}}", 2);
            if (end == std::string_view::npos) {
                throw CheckError(context + "unterminated '{{' in pattern");
            }
            const std::string_view regex = rest.substr(2, end - 2);
            if (regex.empty()) {
                throw CheckError(context + "empty '{{}}' regex in pattern");
            }
            flush_literal();
            pieces.push_back(Piece{Piece::Type::Regex, std::string(regex), {}});
            i += end + 2;
        } else if (rest.starts_with("[[")) {
            const std::size_t end = find_variable_end(rest);
            if (end == std::string_view::npos) {
                throw CheckError(context + "unterminated '[[' in pattern");
            }
            const std::string_view body = rest.substr(2, end - 2);
            const std::size_t colon = body.find(':');
            const std::string_view name = body.substr(0, colon);
            if (!is_valid_name(name)) {
                throw CheckError(context + "invalid variable name '" + std::string(name) + "'");
            }
            flush_literal();
            if (colon == std::string_view::npos) {
                pieces.push_back(Piece{Piece::Type::Use, {}, std::string(name)});
            } else {
                const std::string_view regex = body.substr(colon + 1);
                if (regex.empty()) {
                    throw CheckError(context + "variable '" + std::string(name) +
                                     "' has an empty regex");
                }
                pieces.push_back(Piece{Piece::Type::Define, std::string(regex), std::string(name)});
            }
            i += end + 2;
        } else {
            literal += pattern[i];
            ++i;
        }
    }
    flush_literal();
    return pieces;
}

std::vector<Directive> parse_checks(std::string_view checks, const std::string& prefix) {
    static const std::map<std::string, Kind, std::less<>> kSuffixes = {
        {"NEXT", Kind::Next}, {"SAME", Kind::Same}, {"EMPTY", Kind::Empty},
        {"NOT", Kind::Not},   {"DAG", Kind::Dag},
    };
    std::vector<Directive> directives;
    std::size_t line_number = 0;
    bool have_positive = false;
    while (!checks.empty()) {
        ++line_number;
        const std::size_t newline = checks.find('\n');
        const std::string_view line = checks.substr(0, newline);
        checks =
            newline == std::string_view::npos ? std::string_view{} : checks.substr(newline + 1);

        std::size_t search = 0;
        while (true) {
            const std::size_t at = line.find(prefix, search);
            if (at == std::string_view::npos) {
                break;
            }
            search = at + 1;
            if (at > 0 && is_word_char(line[at - 1])) {
                continue; // e.g. "MYCHECK:" or "X-CHECK:"
            }
            const std::string where_line = "check line " + std::to_string(line_number) + ": ";
            std::size_t cursor = at + prefix.size();
            Kind kind = Kind::Plain;
            const char next = cursor < line.size() ? line[cursor] : '\0';
            if (next == ':') {
                // plain directive
            } else if (next == '-') {
                std::size_t end = cursor + 1;
                while (end < line.size() && ((line[end] >= 'A' && line[end] <= 'Z') ||
                                             (line[end] >= 'a' && line[end] <= 'z'))) {
                    ++end;
                }
                const std::string_view suffix = line.substr(cursor + 1, end - cursor - 1);
                std::string upper(suffix);
                for (char& c : upper) {
                    if (c >= 'a' && c <= 'z') {
                        c = static_cast<char>(c - 'a' + 'A');
                    }
                }
                const std::string spelled = prefix + "-" + std::string(suffix);
                if (upper == "COUNT" || upper == "LABEL") {
                    throw CheckError(where_line + "directive '" + prefix + "-" + upper +
                                     "' is not supported by this matcher");
                }
                const auto known = kSuffixes.find(upper);
                if (known == kSuffixes.end()) {
                    throw CheckError(where_line + "unknown directive '" + spelled + "'");
                }
                if (upper != suffix) {
                    throw CheckError(where_line + "directive '" + spelled + "' must be spelled '" +
                                     prefix + "-" + upper + "'");
                }
                if (end >= line.size() || line[end] != ':') {
                    throw CheckError(where_line + "directive '" + spelled +
                                     "' must be followed directly by ':'");
                }
                kind = known->second;
                cursor = end;
            } else if (is_word_char(next)) {
                continue; // "CHECKS", "CHECK_X": a different word, not a directive
            } else {
                // "CHECK foo", "CHECK :", "CHECK" at the end of a line, "CHECK.": almost
                // certainly a broken directive, never silently ignored.
                throw CheckError(where_line + "'" + prefix +
                                 "' must be followed directly by ':' or '-<KIND>:'");
            }
            const std::string context = where(line_number, kind, prefix);
            const std::string_view pattern = trim(line.substr(cursor + 1));
            if (kind == Kind::Empty) {
                if (!pattern.empty()) {
                    throw CheckError(context + "takes no pattern");
                }
            } else if (pattern.empty()) {
                throw CheckError(context + "empty pattern");
            }
            if ((kind == Kind::Next || kind == Kind::Same || kind == Kind::Empty) &&
                !have_positive) {
                throw CheckError(context + "needs a previous positive match (CHECK/-DAG/...)");
            }
            Directive directive{kind, line_number, std::string(pattern), {}};
            directive.pieces = parse_pattern(pattern, context);
            if (kind == Kind::Not) {
                for (const Piece& piece : directive.pieces) {
                    if (piece.type == Piece::Type::Define) {
                        throw CheckError(context + "cannot define variables");
                    }
                }
            } else {
                have_positive = true;
            }
            directives.push_back(std::move(directive));
            break; // one directive per line
        }
    }
    if (directives.empty()) {
        throw CheckError("no '" + prefix + ":' directives found in the check text");
    }
    return directives;
}

void append_escaped(std::string& out, std::string_view text, bool flexible_whitespace) {
    static constexpr std::string_view kSpecial = "\\^$.|?*+()[]{}";
    for (std::size_t i = 0; i < text.size(); ++i) {
        const char c = text[i];
        if (flexible_whitespace && is_horizontal_space(c)) {
            while (i + 1 < text.size() && is_horizontal_space(text[i + 1])) {
                ++i;
            }
            out += "[ \\t]+";
        } else {
            if (kSpecial.find(c) != std::string_view::npos) {
                out += '\\';
            }
            out += c;
        }
    }
}

std::size_t capture_count(const std::string& regex, const std::string& context) {
    try {
        return std::regex(regex, std::regex::ECMAScript).mark_count();
    } catch (const std::regex_error& e) {
        throw CheckError(context + "invalid regex '" + regex + "': " + e.what());
    }
}

struct CompiledPattern {
    std::regex regex;
    std::vector<std::pair<std::string, std::size_t>> definitions; // name -> capture group
    std::string substitutions;                                    // for messages: NAME="value", ...
};

CompiledPattern compile(const Directive& directive, const std::map<std::string, std::string>& vars,
                        const std::string& context) {
    CompiledPattern compiled;
    std::string regex;
    std::size_t groups = 0;
    for (const Piece& piece : directive.pieces) {
        switch (piece.type) {
        case Piece::Type::Literal:
            append_escaped(regex, piece.text, true);
            break;
        case Piece::Type::Regex:
            groups += capture_count(piece.text, context);
            regex += "(?:" + piece.text + ")";
            break;
        case Piece::Type::Define: {
            const std::size_t inner = capture_count(piece.text, context);
            regex += "(" + piece.text + ")";
            ++groups;
            compiled.definitions.emplace_back(piece.name, groups);
            groups += inner;
            break;
        }
        case Piece::Type::Use: {
            const bool defined_here =
                std::any_of(compiled.definitions.begin(), compiled.definitions.end(),
                            [&](const auto& def) { return def.first == piece.name; });
            if (defined_here) {
                throw CheckError(context + "variable '" + piece.name +
                                 "' is used in the pattern that defines it (unsupported)");
            }
            const auto it = vars.find(piece.name);
            if (it == vars.end()) {
                throw CheckError(context + "undefined variable '" + piece.name + "'");
            }
            append_escaped(regex, it->second, false);
            if (!compiled.substitutions.empty()) {
                compiled.substitutions += ", ";
            }
            compiled.substitutions += piece.name + "=\"" + it->second + "\"";
            break;
        }
        }
    }
    try {
        compiled.regex = std::regex(regex, std::regex::ECMAScript);
    } catch (const std::regex_error& e) {
        throw CheckError(context + "invalid pattern regex '" + regex + "': " + e.what());
    }
    return compiled;
}

struct Line {
    std::size_t begin;
    std::size_t end; // exclusive, without '\n'
};

struct Match {
    std::size_t begin;
    std::size_t end;
    std::size_t line;
    std::vector<std::pair<std::string, std::string>> bindings;
};

class Matcher {
public:
    Matcher(std::string_view input, std::string prefix)
        : input_(input), prefix_(std::move(prefix)) {
        std::size_t begin = 0;
        while (true) {
            const std::size_t newline = input.find('\n', begin);
            if (newline == std::string_view::npos) {
                if (begin < input.size()) {
                    lines_.push_back(Line{begin, input.size()});
                }
                break;
            }
            lines_.push_back(Line{begin, newline});
            begin = newline + 1;
        }
    }

    void run(const std::vector<Directive>& directives);

private:
    std::string context(const Directive& d) const { return where(d.check_line, d.kind, prefix_); }

    std::size_t line_of(std::size_t offset) const {
        const auto it = std::upper_bound(lines_.begin(), lines_.end(), offset,
                                         [](std::size_t o, const Line& l) { return o < l.begin; });
        return it == lines_.begin() ? 0 : static_cast<std::size_t>(it - lines_.begin()) - 1;
    }

    std::string show_line(std::size_t line) const {
        if (line >= lines_.size()) {
            return "<end of input>";
        }
        return "\"" +
               std::string(
                   input_.substr(lines_[line].begin, lines_[line].end - lines_[line].begin)) +
               "\"";
    }

    // First match of `pattern` within [from, to), restricted to single lines.
    std::optional<Match> search(const CompiledPattern& pattern, std::size_t from,
                                std::size_t to) const;

    // Fails if any pending CHECK-NOT pattern occurs in [from, to).
    void check_nots(std::size_t from, std::size_t to);

    void bind(const Match& match) {
        for (const auto& [name, value] : match.bindings) {
            vars_[name] = value;
        }
    }

    [[noreturn]] void fail(const Directive& d, const CompiledPattern& p, const std::string& what) {
        std::string message = context(d) + what + "\n  pattern: " + d.pattern;
        if (!p.substitutions.empty()) {
            message += "\n  with: " + p.substitutions;
        }
        throw CheckError(message);
    }

    std::string_view input_;
    std::string prefix_;
    std::vector<Line> lines_;
    std::map<std::string, std::string> vars_;
    std::vector<const Directive*> pending_nots_;
};

std::optional<Match> Matcher::search(const CompiledPattern& pattern, std::size_t from,
                                     std::size_t to) const {
    if (lines_.empty() || from > to) {
        return std::nullopt;
    }
    for (std::size_t l = line_of(from); l < lines_.size() && lines_[l].begin <= to; ++l) {
        const std::size_t seg_begin = std::max(lines_[l].begin, from);
        const std::size_t seg_end = std::min(lines_[l].end, to);
        if (seg_begin > seg_end) {
            continue;
        }
        auto flags = std::regex_constants::match_default;
        if (seg_begin > lines_[l].begin) {
            flags |= std::regex_constants::match_prev_avail;
        }
        if (seg_end < lines_[l].end) {
            flags |= std::regex_constants::match_not_eol;
        }
        const char* base = input_.data();
        std::cmatch m;
        if (std::regex_search(base + seg_begin, base + seg_end, m, pattern.regex, flags)) {
            Match match;
            match.begin = seg_begin + static_cast<std::size_t>(m.position(0));
            match.end = match.begin + static_cast<std::size_t>(m.length(0));
            match.line = l;
            for (const auto& [name, group] : pattern.definitions) {
                match.bindings.emplace_back(name, m[group].str());
            }
            return match;
        }
    }
    return std::nullopt;
}

void Matcher::check_nots(std::size_t from, std::size_t to) {
    for (const Directive* d : pending_nots_) {
        const CompiledPattern pattern = compile(*d, vars_, context(*d));
        if (const auto hit = search(pattern, from, to)) {
            fail(*d, pattern,
                 "excluded string found on input line " + std::to_string(hit->line + 1) + ": " +
                     show_line(hit->line));
        }
    }
    pending_nots_.clear();
}

void Matcher::run(const std::vector<Directive>& directives) {
    std::size_t pos = 0;                      // end of the previous positive match
    std::optional<std::size_t> previous_line; // line of the previous positive match
    for (std::size_t i = 0; i < directives.size(); ++i) {
        const Directive& d = directives[i];
        switch (d.kind) {
        case Kind::Not:
            pending_nots_.push_back(&d);
            break;
        case Kind::Dag: {
            std::vector<std::pair<std::size_t, std::size_t>> taken; // matched ranges of this run
            // Bindings of this run become visible to later DAGs of the run at once, but to
            // CHECK-NOT patterns before the run only after those have been checked.
            std::map<std::string, std::string> run_vars = vars_;
            std::size_t run_begin = input_.size();
            std::size_t run_end = pos;
            std::size_t run_end_line = previous_line.value_or(0);
            for (; i < directives.size() && directives[i].kind == Kind::Dag; ++i) {
                const Directive& dag = directives[i];
                const CompiledPattern pattern = compile(dag, run_vars, context(dag));
                std::optional<Match> found;
                for (std::size_t from = pos; from <= input_.size();) {
                    found = search(pattern, from, input_.size());
                    if (!found) {
                        break;
                    }
                    const bool overlaps = std::any_of(taken.begin(), taken.end(), [&](auto r) {
                        return found->begin < r.second && r.first < found->end;
                    });
                    if (!overlaps) {
                        break;
                    }
                    from = found->begin + 1;
                    found.reset();
                }
                if (!found) {
                    fail(dag, pattern,
                         "expected string not found (searched from input line " +
                             std::to_string(line_of(pos) + 1) + ", without overlapping other " +
                             prefix_ + "-DAG matches)");
                }
                taken.emplace_back(found->begin, found->end);
                run_begin = std::min(run_begin, found->begin);
                if (found->end >= run_end) {
                    run_end = found->end;
                    run_end_line = found->line;
                }
                for (const auto& [name, value] : found->bindings) {
                    run_vars[name] = value;
                }
            }
            --i; // the for loop's ++i moves to the first directive after the run
            check_nots(pos, run_begin);
            vars_ = std::move(run_vars);
            pos = run_end;
            previous_line = run_end_line;
            break;
        }
        case Kind::Plain:
        case Kind::Next:
        case Kind::Same:
        case Kind::Empty: {
            const CompiledPattern pattern = compile(d, vars_, context(d));
            std::optional<Match> found;
            if (d.kind == Kind::Plain) {
                found = search(pattern, pos, input_.size());
                if (!found) {
                    fail(d, pattern,
                         "expected string not found (searched from input line " +
                             std::to_string(line_of(pos) + 1) + ")");
                }
            } else {
                // Parsing guarantees a previous positive directive.
                const std::size_t line = previous_line.value_or(0);
                if (d.kind == Kind::Next) {
                    // LLVM semantics: the first match after the previous one must be on the
                    // next line (a match on the same line is an error, not skipped).
                    found = search(pattern, pos, input_.size());
                    if (!found) {
                        fail(d, pattern,
                             "expected string not found (searched from input line " +
                                 std::to_string(line_of(pos) + 1) + ")");
                    }
                    if (found->line != line + 1) {
                        fail(d, pattern,
                             "first match is on input line " + std::to_string(found->line + 1) +
                                 (found->line == line ? " (the same line as the previous match)"
                                                      : "") +
                                 ", expected on the next line " + std::to_string(line + 2) + ": " +
                                 show_line(found->line));
                    }
                } else if (d.kind == Kind::Same) {
                    found = search(pattern, pos, lines_[line].end);
                    if (!found) {
                        fail(d, pattern,
                             "expected string not found later on the same input line " +
                                 std::to_string(line + 1) + ": " + show_line(line));
                    }
                } else { // Kind::Empty
                    const std::size_t target = line + 1;
                    if (target >= lines_.size()) {
                        fail(d, pattern,
                             "no input line " + std::to_string(target + 1) +
                                 " after the previous match on line " + std::to_string(line + 1));
                    }
                    if (lines_[target].begin != lines_[target].end) {
                        fail(d, pattern,
                             "input line " + std::to_string(target + 1) +
                                 " is not empty: " + show_line(target));
                    }
                    found = Match{lines_[target].begin, lines_[target].begin, target, {}};
                }
            }
            check_nots(pos, found->begin);
            bind(*found);
            pos = found->end;
            previous_line = found->line;
            break;
        }
        }
    }
    check_nots(pos, input_.size());
}

} // namespace

FileCheckResult filecheck(std::string_view checks, std::string_view input,
                          const FileCheckOptions& options) {
    FileCheckResult result;
    try {
        if (options.prefix.empty() ||
            !std::all_of(options.prefix.begin(), options.prefix.end(), is_word_char)) {
            throw CheckError("invalid check prefix '" + options.prefix + "'");
        }
        const std::vector<Directive> directives = parse_checks(checks, options.prefix);
        Matcher matcher(input, options.prefix);
        matcher.run(directives);
        result.ok = true;
    } catch (const CheckError& e) {
        result.message = e.what();
    }
    return result;
}

} // namespace a2s_test
