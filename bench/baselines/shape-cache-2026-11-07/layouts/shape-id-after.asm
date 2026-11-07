
artifacts/performance/shape-final-build-2/comparison:     file format elf64-x86-64


Disassembly of section .text:

00000000010c9b70 <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id>:
 10c9b70:	48 8b 8f c0 01 00 00 	mov    0x1c0(%rdi),%rcx
 10c9b77:	48 b8 ff ff ff ff ff 	movabs $0x7fffffffffffffff,%rax
 10c9b7e:	ff ff 7f 
 10c9b81:	48 39 c1             	cmp    %rax,%rcx
 10c9b84:	73 34                	jae    10c9bba <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id+0x4a>
 10c9b86:	48 8d 41 01          	lea    0x1(%rcx),%rax
 10c9b8a:	48 89 87 c0 01 00 00 	mov    %rax,0x1c0(%rdi)
 10c9b91:	48 8b 87 c8 01 00 00 	mov    0x1c8(%rdi),%rax
 10c9b98:	48 85 c0             	test   %rax,%rax
 10c9b9b:	74 13                	je     10c9bb0 <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id+0x40>
 10c9b9d:	8b 90 98 00 00 00    	mov    0x98(%rax),%edx
 10c9ba3:	b8 01 00 00 00       	mov    $0x1,%eax
 10c9ba8:	48 89 8f c0 01 00 00 	mov    %rcx,0x1c0(%rdi)
 10c9baf:	c3                   	ret
 10c9bb0:	31 c0                	xor    %eax,%eax
 10c9bb2:	48 89 8f c0 01 00 00 	mov    %rcx,0x1c0(%rdi)
 10c9bb9:	c3                   	ret
 10c9bba:	50                   	push   %rax
 10c9bbb:	48 8d 3d be fe 63 00 	lea    0x63febe(%rip),%rdi        # 1709a80 <anon.885cf56f9efe57f6b6b5ddb3202e50ac.2129.llvm.10559845329444439404+0x3728>
 10c9bc2:	ff 15 20 b8 6b 00    	call   *0x6bb820(%rip)        # 17853e8 <_DYNAMIC+0x508>

Disassembly of section .init:

Disassembly of section .fini:

Disassembly of section .plt:
