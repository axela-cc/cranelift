"""
Generate sources for instruction encoding.

The tables and functions generated here support the `TargetIsa::encode()`
function which determines if a given instruction is legal, and if so, it's
`Encoding` data which consists of a *recipe* and some *encoding* bits.

The `encode` function doesn't actually generate the binary machine bits. Each
recipe has a corresponding hand-written function to do that after registers
are allocated.

This is the information available to us:

- The instruction to be encoded as an `Inst` reference.
- The data-flow graph containing the instruction, giving us access to the
  `InstructionData` representation and the types of all values involved.
- A target ISA instance with shared and ISA-specific settings for evaluating
  ISA predicates.
- The currently active CPU mode is determined by the ISA.

## Level 1 table lookup

The CPU mode provides the first table. The key is the instruction's controlling
type variable. If the instruction is not polymorphic, use `VOID` for the type
variable. The table values are level 2 tables.

## Level 2 table lookup

The level 2 table is keyed by the instruction's opcode. The table values are
*encoding lists*.

The two-level table lookup allows the level 2 tables to be much smaller with
good locality. Code in any given function usually only uses a few different
types, so many of the level 2 tables will be cold.

## Encoding lists

An encoding list is a non-empty sequence of list entries. Each entry has
one of these forms:

1. Instruction predicate, encoding recipe, and encoding bits. If the
   instruction predicate is true, use this recipe and bits.
2. ISA predicate and skip-count. If the ISA predicate is false, skip the next
   *skip-count* entries in the list. If the skip count is zero, stop
   completely.
3. Stop. End of list marker. If this is reached, the instruction does not have
   a legal encoding.

The instruction predicate is also used to distinguish between polymorphic
instructions with different types for secondary type variables.
"""
from __future__ import absolute_import
import srcgen
from unique_table import UniqueSeqTable
from collections import OrderedDict, defaultdict


def emit_instp(instp, fmt):
    """
    Emit code for matching an instruction predicate against an
    `InstructionData` reference called `inst`.

    The generated code is a pattern match that falls through if the instruction
    has an unexpected format. This should lead to a panic.
    """
    iform = instp.predicate_context()

    # Which fiels do we need in the InstructionData pattern match?
    if iform.boxed_storage:
        fields = 'ref data'
    else:
        # Collect the leaf predicates
        leafs = set()
        instp.predicate_leafs(leafs)
        # All the leafs are FieldPredicate instances. Here we just care about
        # the field names.
        fields = ', '.join(sorted(set(p.field.name for p in leafs)))

    with fmt.indented('{} => {{'.format(instp.number), '}'):
        with fmt.indented(
                'if let {} {{ {}, .. }} = *inst {{'
                .format(iform.name, fields), '}'):
            fmt.line('return {};'.format(instp.rust_predicate(0)))


def emit_instps(instps, fmt):
    """
    Emit a function for matching instruction predicates.
    """

    with fmt.indented(
            'fn check_instp(inst: &InstructionData, instp_idx: u16) -> bool {',
            '}'):
        with fmt.indented('match instp_idx {', '}'):
            for instp in instps:
                emit_instp(instp, fmt)
            fmt.line('_ => panic!("Invalid instruction predicate")')

        # The match cases will fall through if the instruction format is wrong.
        fmt.line('panic!("Bad format {}/{} for instp {}",')
        fmt.line('       InstructionFormat::from(inst),')
        fmt.line('       inst.opcode(),')
        fmt.line('       instp_idx);')


# Encoding lists are represented as u16 arrays.
CODE_BITS = 16
PRED_BITS = 12
PRED_MASK = (1 << PRED_BITS) - 1

# 0..CODE_ALWAYS means: Check instruction predicate and use the next two
# entries as a (recipe, encbits) pair if true. CODE_ALWAYS is the always-true
# predicate, smaller numbers refer to instruction predicates.
CODE_ALWAYS = PRED_MASK

# Codes above CODE_ALWAYS indicate an ISA predicate to be tested.
# `x & PRED_MASK` is the ISA predicate number to test.
# `(x >> PRED_BITS)*3` is the number of u16 table entries to skip if the ISA
# predicate is false. (The factor of three corresponds to the (inst-pred,
# recipe, encbits) triples.
#
# Finally, CODE_FAIL indicates the end of the list.
CODE_FAIL = (1 << CODE_BITS) - 1


def seq_doc(enc):
    """
    Return a tuple containing u16 representations of the instruction predicate
    an recipe / encbits.

    Also return a doc string.
    """
    if enc.instp:
        p = enc.instp.number
        doc = '--> {} when {}'.format(enc, enc.instp)
    else:
        p = CODE_ALWAYS
        doc = '--> {}'.format(enc)
    assert p <= CODE_ALWAYS
    return ((p, enc.recipe.number, enc.encbits), doc)


class EncList(object):
    """
    List of instructions for encoding a given type + opcode pair.

    An encoding list contains a sequence of predicates and encoding recipes,
    all encoded as u16 values.

    :param inst: The instruction opcode being encoded.
    :param ty: Value of the controlling type variable, or `None`.
    """

    def __init__(self, inst, ty):
        self.inst = inst
        self.ty = ty
        # List of applicable Encoding instances.
        # These will have different predicates.
        self.encodings = []

    def name(self):
        name = self.inst.name
        if self.ty:
            name = '{}.{}'.format(name, self.ty.name)
        if self.encodings:
            name += ' ({})'.format(self.encodings[0].cpumode)
        return name

    def encode(self, seq_table, doc_table):
        """
        Encode this list as a sequence of u16 numbers.

        Adds the sequence to `seq_table` and records the returned offset as
        `self.offset`.

        Adds comment lines to `doc_table` keyed by seq_table offsets.
        """
        words = list()
        docs = list()

        for idx, enc in enumerate(self.encodings):
            seq, doc = seq_doc(enc)
            docs.append((len(words), doc))
            words.extend(seq)
        words.append(CODE_FAIL)

        self.offset = seq_table.add(words)

        # Add doc comments.
        doc_table[self.offset].append(
                '{:06x}: {}'.format(self.offset, self.name()))
        for pos, doc in docs:
            doc_table[self.offset + pos].append(doc)


class Level2Table(object):
    """
    Level 2 table mapping instruction opcodes to `EncList` objects.

    :param ty: Controlling type variable of all entries, or `None`.
    """

    def __init__(self, ty):
        self.ty = ty
        # Maps inst -> EncList
        self.lists = OrderedDict()

    def __getitem__(self, inst):
        ls = self.lists.get(inst)
        if not ls:
            ls = EncList(inst, self.ty)
            self.lists[inst] = ls
        return ls

    def __iter__(self):
        return iter(self.lists.values())


class Level1Table(object):
    """
    Level 1 table mapping types to `Level2` objects.
    """

    def __init__(self):
        self.tables = OrderedDict()

    def __getitem__(self, ty):
        tbl = self.tables.get(ty)
        if not tbl:
            tbl = Level2Table(ty)
            self.tables[ty] = tbl
        return tbl

    def __iter__(self):
        return iter(self.tables.values())


def make_tables(cpumode):
    """
    Generate tables for `cpumode` as described above.
    """
    table = Level1Table()
    for enc in cpumode.encodings:
        ty = enc.ctrl_typevar()
        inst = enc.inst
        table[ty][inst].encodings.append(enc)
    return table


def encode_enclists(level1, seq_table, doc_table):
    """
    Compute encodings and doc comments for encoding lists in `level1`.
    """
    for level2 in level1:
        for enclist in level2:
            enclist.encode(seq_table, doc_table)


def emit_enclists(seq_table, doc_table, fmt):
    with fmt.indented(
            'const ENCLISTS: [u16; {}] = ['.format(len(seq_table.table)),
            '];'):
        line = ''
        for idx, entry in enumerate(seq_table.table):
            if idx in doc_table:
                if line:
                    fmt.line(line)
                    line = ''
                for doc in doc_table[idx]:
                    fmt.comment(doc)
            line += '{:#06x}, '.format(entry)
        if line:
            fmt.line(line)


def gen_isa(isa, fmt):
    # First assign numbers to relevant instruction predicates and generate the
    # check_instp() function..
    emit_instps(isa.all_instps, fmt)

    # Tables for enclists with comments.
    seq_table = UniqueSeqTable()
    doc_table = defaultdict(list)

    for cpumode in isa.cpumodes:
        level1 = make_tables(cpumode)
        encode_enclists(level1, seq_table, doc_table)

    emit_enclists(seq_table, doc_table, fmt)


def generate(isas, out_dir):
    for isa in isas:
        fmt = srcgen.Formatter()
        gen_isa(isa, fmt)
        fmt.update_file('encoding-{}.rs'.format(isa.name), out_dir)