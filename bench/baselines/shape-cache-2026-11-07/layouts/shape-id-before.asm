
artifacts/performance/memory-observed-build/comparison:     file format elf64-x86-64


Disassembly of section .text:

00000000010c9ed0 <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id>:
 10c9ed0:	48 8b 8f c0 01 00 00 	mov    0x1c0(%rdi),%rcx
 10c9ed7:	48 b8 ff ff ff ff ff 	movabs $0x7fffffffffffffff,%rax
 10c9ede:	ff ff 7f 
 10c9ee1:	48 39 c1             	cmp    %rax,%rcx
 10c9ee4:	73 34                	jae    10c9f1a <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id+0x4a>
 10c9ee6:	48 8d 41 01          	lea    0x1(%rcx),%rax
 10c9eea:	48 89 87 c0 01 00 00 	mov    %rax,0x1c0(%rdi)
 10c9ef1:	48 8b 87 c8 01 00 00 	mov    0x1c8(%rdi),%rax
 10c9ef8:	48 85 c0             	test   %rax,%rax
 10c9efb:	74 13                	je     10c9f10 <_RNvMs4_NtCs9SuVsbL0D91_12napi_vm_core5valueNtB5_10ObjectCell8shape_id+0x40>
 10c9efd:	8b 90 90 00 00 00    	mov    0x90(%rax),%edx
 10c9f03:	b8 01 00 00 00       	mov    $0x1,%eax
 10c9f08:	48 89 8f c0 01 00 00 	mov    %rcx,0x1c0(%rdi)
 10c9f0f:	c3                   	ret
 10c9f10:	31 c0                	xor    %eax,%eax
 10c9f12:	48 89 8f c0 01 00 00 	mov    %rcx,0x1c0(%rdi)
 10c9f19:	c3                   	ret
 10c9f1a:	50                   	push   %rax
 10c9f1b:	48 8d 3d 96 e4 63 00 	lea    0x63e496(%rip),%rdi        # 17083b8 <anon.0ac84d084868b117dd78981987e6b8ef.2129.llvm.17548735469515057753+0x3728>
 10c9f22:	ff 15 80 9c 6b 00    	call   *0x6b9c80(%rip)        # 1783ba8 <_DYNAMIC+0x508>

Disassembly of section .init:

Disassembly of section .fini:

Disassembly of section .plt:
